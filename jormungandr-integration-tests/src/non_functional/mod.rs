pub mod compatibility;
/*
 Explorer soak test. Run node for ~15 minutes and verify explorer is in sync with node rest
*/
pub mod explorer;
/*
 Sanity performance tests. Quick tests to check overall node performance.
 Run some transaction for ~15 minutes or specified no of transactions (100)
*/
pub mod sanity;
/*
Long running test for self node (48 h)
*/
pub mod soak;

use crate::common::{
    explorer::ExplorerError,
    jcli_wrapper,
    jormungandr::{JormungandrError, JormungandrProcess},
    process_utils,
};
use jormungandr_lib::{crypto::hash::Hash, interfaces::Value, wallet::Wallet};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NodeStuckError {
    #[error("node tip is not moving up. Stuck at {tip_hash} ")]
    TipIsNotMoving { tip_hash: String, logs: String },
    #[error("node block counter is not moving up. Stuck at {block_counter}")]
    BlockCounterIsNoIncreased { block_counter: u64, logs: String },
    #[error("accounts funds were not trasfered (actual: {actual} vs expected: {expected}). Logs: {logs}")]
    FundsNotTransfered {
        actual: Value,
        expected: Value,
        logs: String,
    },
    #[error("explorer is out of sync with rest node (actual: {actual} vs expected: {expected}). Logs: {logs}")]
    ExplorerTipIsOutOfSync {
        actual: Hash,
        expected: Hash,
        logs: String,
    },
    #[error("error in logs found")]
    InternalJormungandrError(#[from] JormungandrError),
    #[error("jcli error")]
    InternalJcliError(#[from] jcli_wrapper::Error),
    #[error("exploer error")]
    InternalExplorerError(#[from] ExplorerError),
}

pub fn send_transaction_and_ensure_block_was_produced(
    transation_messages: &Vec<String>,
    jormungandr: &JormungandrProcess,
) -> Result<(), NodeStuckError> {
    let block_tip_before_transaction =
        jcli_wrapper::assert_rest_get_block_tip(&jormungandr.rest_address());
    let block_counter_before_transaction = jormungandr.logger.get_created_blocks_counter();

    jcli_wrapper::send_transactions_and_wait_until_in_block(&transation_messages, &jormungandr)
        .map_err(|err| NodeStuckError::InternalJcliError(err))?;

    let block_tip_after_transaction =
        jcli_wrapper::assert_rest_get_block_tip(&jormungandr.rest_address());
    let block_counter_after_transaction = jormungandr.logger.get_created_blocks_counter();

    if block_tip_before_transaction == block_tip_after_transaction {
        return Err(NodeStuckError::TipIsNotMoving {
            tip_hash: block_tip_after_transaction.clone(),
            logs: jormungandr.logger.get_log_content(),
        });
    }

    if block_counter_before_transaction == block_counter_after_transaction {
        return Err(NodeStuckError::BlockCounterIsNoIncreased {
            block_counter: block_counter_before_transaction as u64,
            logs: jormungandr.logger.get_log_content(),
        });
    }

    Ok(())
}

pub fn check_transaction_was_processed(
    transaction: String,
    receiver: &Wallet,
    value: u64,
    jormungandr: &JormungandrProcess,
) -> Result<(), NodeStuckError> {
    send_transaction_and_ensure_block_was_produced(&vec![transaction], &jormungandr)?;

    check_funds_transferred_to(&receiver.address().to_string(), value.into(), &jormungandr)?;

    jormungandr
        .check_no_errors_in_log()
        .map_err(|err| NodeStuckError::InternalJormungandrError(err))
}

pub fn assert_nodes_are_in_sync(nodes: Vec<&JormungandrProcess>) {
    if nodes.len() < 2 {
        return;
    }
    let sync_wait: u64 = (nodes.len() * 10) as u64;
    process_utils::sleep(sync_wait);
    let first_node = nodes.iter().next().unwrap();
    let block_height = first_node
        .rest()
        .stats()
        .unwrap()
        .stats
        .unwrap()
        .last_block_height
        .unwrap()
        .parse::<i32>()
        .unwrap();
    let grace_value = 2;

    for node in nodes.iter().skip(1) {
        let current_block_height = &node
            .rest()
            .stats()
            .unwrap()
            .stats
            .unwrap()
            .last_block_height
            .unwrap()
            .parse::<i32>()
            .unwrap();
        let abs = (current_block_height - block_height).abs();
        println!("{} vs {}.. {}", block_height, current_block_height, abs);
        assert!(
            abs <= grace_value,
            format!("Nodes are out of sync more than {}", grace_value)
        );
    }
}

pub fn assert_no_errors_in_logs(nodes: Vec<&JormungandrProcess>, message: &str) {
    for node in nodes {
        node.assert_no_errors_in_log_with_message(message);
    }
}

pub fn check_funds_transferred_to(
    address: &str,
    value: Value,
    jormungandr: &JormungandrProcess,
) -> Result<(), NodeStuckError> {
    let account_state =
        jcli_wrapper::assert_rest_account_get_stats(address, &jormungandr.rest_address());

    if *account_state.value() != value {
        return Err(NodeStuckError::FundsNotTransfered {
            actual: account_state.value().clone(),
            expected: value.clone(),
            logs: jormungandr.logger.get_log_content(),
        });
    }
    Ok(())
}
