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

/// Own the exact child processes started by this test. The upstream CLI's
/// localnet launcher stops services by process name, including other sessions.
pub struct LocalnetServices {
    children: Vec<Child>,
    pub rpc_url: String,
    pub indexer_url: String,
    pub prover_url: String,
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
                    == "5330a112cb10e7622585f61cde2397d26721fdb6",
            "prepare the pinned Zolana checkout first"
        );
        let offset: u16 = std::env::var("ESCROW_PORT_OFFSET")
            .unwrap_or_else(|_| "10000".into())
            .parse()?;
        let ports = [8899u16, 8900, 9900, 8784, 3001, 9998].map(|p| p.checked_add(offset));
        let ports: Vec<u16> = ports
            .into_iter()
            .collect::<Option<_>>()
            .context("port offset overflow")?;
        // Hold all reservations until immediately before spawning services.
        let reservations = ports
            .iter()
            .map(|p| {
                TcpListener::bind(("127.0.0.1", *p))
                    .with_context(|| format!("test port {p} is occupied"))
            })
            .collect::<Result<Vec<_>>>()?;
        let run = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let logs = base.join("target/localnet").join(run);
        fs::create_dir_all(&logs)?;
        smart_account::write_program_config_fixture(
            logs.join("accounts").to_str().context("account path")?,
        );
        let mut services = Self {
            children: vec![],
            rpc_url: format!("http://127.0.0.1:{}", ports[0]),
            indexer_url: format!("http://127.0.0.1:{}", ports[3]),
            prover_url: format!("http://127.0.0.1:{}", ports[4]),
            logs,
        };
        let mut validator = Command::new("solana-test-validator");
        validator
            .args([
                "--quiet",
                "--limit-ledger-size",
                "10000",
                "--bind-address",
                "127.0.0.1",
                "--rpc-port",
                &ports[0].to_string(),
                "--faucet-port",
                &ports[2].to_string(),
            ])
            .arg("--ledger")
            .arg(services.logs.join("ledger"))
            .arg("--account-dir")
            .arg(services.logs.join("accounts"));
        for (id, binary) in [
            (
                timelock_escrow_program::ID.to_string(),
                base.join("target/deploy/timelock_escrow_program.so"),
            ),
            (
                solana_pubkey::Pubkey::new_from_array(SHIELDED_POOL_PROGRAM_ID).to_string(),
                root.join("target/deploy/shielded_pool_program.so"),
            ),
            (
                smart_account::SMART_ACCOUNT_PROGRAM_ID.to_string(),
                root.join("target/deploy/squads_smart_account_program.so"),
            ),
        ] {
            anyhow::ensure!(binary.is_file(), "missing program: {}", binary.display());
            validator.arg("--bpf-program").arg(id).arg(binary);
        }
        drop(reservations);
        services.spawn(validator, "validator")?;
        services.wait_http(&format!("{}/health", services.rpc_url))?;

        let mut photon = Command::new(root.join("target/debug/photon"));
        photon
            .args([
                "--rpc-url",
                &services.rpc_url,
                "--port",
                &ports[3].to_string(),
                "--start-slot",
                "0",
            ])
            .current_dir(&services.logs);
        services.spawn(photon, "photon")?;
        services.wait_http(&format!("{}/readiness", services.indexer_url))?;

        let keys = root.join("prover/server/proving-keys");
        fs::create_dir_all(&keys)?;
        let mut prover = Command::new(root.join("target/prover-server"));
        prover
            .args([
                "start",
                "--keys-dir",
                &format!("{}/", keys.display()),
                "--prover-address",
                &format!("127.0.0.1:{}", ports[4]),
                "--metrics-address",
                &format!("127.0.0.1:{}", ports[5]),
                "--auto-download=true",
            ])
            .current_dir(&root);
        services.spawn(prover, "prover")?;
        services.wait_http(&format!("{}/health", services.prover_url))?;
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
}
