use anyhow::{bail, Context, Result};
use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zolana_interface::SHIELDED_POOL_PROGRAM_ID;
use zolana_test_utils::smart_account;

/// Own the exact localnet processes started by this test and stop them on drop.
pub struct LocalnetServices {
    children: Vec<Child>,
    pub rpc_url: String,
    pub indexer_url: String,
    logs: PathBuf,
}

impl Drop for LocalnetServices {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl LocalnetServices {
    pub fn start() -> Result<Self> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .canonicalize()?;
        let root = base.join("target/zolana");
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()?;
        anyhow::ensure!(
            revision.status.success()
                && String::from_utf8_lossy(&revision.stdout).trim()
                    == "af11be0e8d27702f8e6553320bd5dab52ab79fed",
            "prepare the pinned Zolana checkout first"
        );

        let offset: u16 = std::env::var("ESCROW_PORT_OFFSET")
            .unwrap_or_else(|_| "10000".into())
            .parse()?;
        let ports = [8899u16, 8784, 3001, 9998].map(|port| port.checked_add(offset));
        let ports: Vec<u16> = ports
            .into_iter()
            .collect::<Option<_>>()
            .context("port offset overflow")?;
        let reservations = ports
            .iter()
            .map(|port| {
                TcpListener::bind(("127.0.0.1", *port))
                    .with_context(|| format!("test port {port} is occupied"))
            })
            .collect::<Result<Vec<_>>>()?;

        let run = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let logs = base.join("target/localnet").join(run);
        let accounts = logs.join("accounts");
        fs::create_dir_all(&accounts)?;
        smart_account::write_program_config_fixture(
            accounts.to_str().context("account path is not UTF-8")?,
        );

        let mut services = Self {
            children: Vec::new(),
            rpc_url: format!("http://127.0.0.1:{}", ports[0]),
            indexer_url: format!("http://127.0.0.1:{}", ports[1]),
            logs,
        };

        let escrow_program = base.join("target/deploy/timelock_escrow_program.so");
        let spp_program = root.join("target/deploy/shielded_pool_program.so");
        let smart_account_program = root.join("target/deploy/squads_smart_account_program.so");
        for path in [&escrow_program, &spp_program, &smart_account_program] {
            anyhow::ensure!(path.is_file(), "missing program: {}", path.display());
        }

        let spp_id = solana_pubkey::Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID).to_string();
        let protocol_vault = smart_account::standard_accounts()
            .protocol_vault
            .to_string();
        let mut surfpool = Command::new(root.join("target/tools/surfpool"));
        surfpool
            .args([
                "start",
                "--offline",
                "--no-tui",
                "--no-deploy",
                "--no-studio",
                "--port",
                &ports[0].to_string(),
                "--host",
                "127.0.0.1",
                "--bpf-program",
                &timelock_escrow_program::ID.to_string(),
            ])
            .arg(&escrow_program)
            .args([
                "--bpf-program",
                &smart_account::SMART_ACCOUNT_PROGRAM_ID.to_string(),
            ])
            .arg(&smart_account_program)
            .args(["--upgradeable-program", &spp_id])
            .arg(&spp_program)
            .arg(&protocol_vault)
            .arg("--account-dir")
            .arg(&accounts)
            .current_dir(&root);
        drop(reservations);
        services.spawn(surfpool, "surfpool")?;
        services.wait_rpc()?;

        let mut photon = Command::new(root.join("target/debug/photon"));
        photon
            .args([
                "--rpc-url",
                &services.rpc_url,
                "--port",
                &ports[1].to_string(),
                "--start-slot",
                "latest",
            ])
            .current_dir(&services.logs);
        services.spawn(photon, "photon")?;
        services.wait_http(&format!("{}/readiness", services.indexer_url))?;

        let prover_url = format!("http://127.0.0.1:{}", ports[2]);
        std::env::set_var("ZOLANA_PROVER_URL", &prover_url);
        let keys = root.join("prover/server/proving-keys");
        fs::create_dir_all(&keys)?;
        let mut prover = Command::new(root.join("target/prover-server"));
        prover
            .args([
                "start",
                "--keys-dir",
                &format!("{}/", keys.display()),
                "--prover-address",
                &format!("127.0.0.1:{}", ports[2]),
                "--metrics-address",
                &format!("127.0.0.1:{}", ports[3]),
                "--auto-download=true",
            ])
            .current_dir(&root);
        services.spawn(prover, "prover")?;
        services.wait_http(&format!("{prover_url}/health"))?;

        println!("Localnet logs: {}", services.logs.display());
        Ok(services)
    }

    fn spawn(&mut self, mut command: Command, name: &str) -> Result<()> {
        let log = fs::File::create(self.logs.join(format!("{name}.log")))?;
        let child = command
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .with_context(|| format!("start {name}; run scripts/build-localnet.sh first"))?;
        self.children.push(child);
        Ok(())
    }

    fn wait_http(&mut self, url: &str) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(120) {
            for child in &mut self.children {
                if let Some(status) = child.try_wait()? {
                    bail!(
                        "localnet service exited: {status}; logs: {}",
                        self.logs.display()
                    );
                }
            }
            if Command::new("curl")
                .args(["--fail", "--silent", "--max-time", "2", url])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        bail!("readiness timed out: {url}; logs: {}", self.logs.display())
    }

    fn wait_rpc(&mut self) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(120) {
            for child in &mut self.children {
                if let Some(status) = child.try_wait()? {
                    bail!(
                        "localnet service exited: {status}; logs: {}",
                        self.logs.display()
                    );
                }
            }
            if Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--max-time",
                    "2",
                    "--header",
                    "content-type: application/json",
                    "--data",
                    r#"{"jsonrpc":"2.0","id":1,"method":"getHealth"}"#,
                    &self.rpc_url,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        bail!(
            "RPC readiness timed out: {}; logs: {}",
            self.rpc_url,
            self.logs.display()
        )
    }
}
