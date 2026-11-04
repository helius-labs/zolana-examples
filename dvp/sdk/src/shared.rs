use anyhow::{ensure, Result};
use spr_program::ProgramOwner;
use zolana_client::{BlockingIndexer, BlockingRpc, ZolanaClient};
use zolana_interface::instruction::instruction_data::transact::TransactIxData;
use zolana_transaction::instructions::transact::SppProofInputs;

use crate::err;

/// Proves a transaction whose real inputs are all owned by the DvP PDA under
/// the zero nullifier secret (padding inputs carry their own) through
/// the client's configured prover, and checks that the SPP
/// proof covers `private_tx_hash`, the transaction the DvP proof covers.
pub(crate) fn prove_pda_transact<R: BlockingRpc, I: BlockingIndexer>(
    client: &ZolanaClient<R, I>,
    spp_proof_inputs: SppProofInputs,
    private_tx_hash: &[u8; 32],
) -> Result<TransactIxData> {
    let spp = client
        .prove_transact(spp_proof_inputs, None, &ProgramOwner::nullifier_key())
        .map_err(err)?;
    ensure!(
        &spp.private_tx_hash == private_tx_hash,
        "the SPP proof and the DvP proof cover different transactions"
    );
    Ok(spp)
}

/// The instruction builders read every input from the output tree, so an
/// input `what` must sit in the tree with id `tree_id`.
pub(crate) fn ensure_output_tree(what: &str, input_tree_id: u16, tree_id: u16) -> Result<()> {
    ensure!(
        input_tree_id == tree_id,
        "{what} is in tree {input_tree_id}, not in the output tree {tree_id}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    use zolana_client::{
        prover::tee::{PolicySigners, SignedTeePolicy},
        ProverClient, ZolanaIndexer,
    };

    use super::*;
    use crate::{
        index::fixture::deposit_fixture, instructions::reclaim_dvp::ReclaimDvpTransactionParams,
    };

    #[test]
    fn pda_transact_uses_the_clients_signed_tee_policy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut bytes = [0; 4096];
                        let count = stream.read(&mut bytes).unwrap();
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        return Some(String::from_utf8_lossy(&bytes[..count]).into_owned());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept policy request: {error}"),
                }
            }
            None
        });
        let signers = PolicySigners::new("devnet", vec![[1; 32]], 1).unwrap();
        let policy = SignedTeePolicy::new(format!("{endpoint}/policy.json"), signers).unwrap();
        let indexer = ZolanaIndexer::new(&endpoint);
        let client = ZolanaClient::new(
            indexer.clone(),
            indexer,
            ProverClient::new(endpoint).with_signed_tee(policy),
        );
        let fixture = deposit_fixture();
        let transaction = ReclaimDvpTransactionParams {
            deposit: fixture.deposits[0].clone(),
            payer: fixture.terms.creator,
            refund_destination: None,
            tree_id: fixture.deposits[0].tree_id,
        }
        .build(&fixture.funders[0])
        .unwrap();
        let hash = transaction
            .spp_proof_inputs
            .padding_independent_private_tx_hash()
            .unwrap();
        assert!(prove_pda_transact(&client, transaction.spp_proof_inputs, &hash).is_err());
        let request = server
            .join()
            .unwrap()
            .expect("configured policy was not requested");
        assert!(request.starts_with("GET /policy.json HTTP/1.1\r\n"));
    }
}
