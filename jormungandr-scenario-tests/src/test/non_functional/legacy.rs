use crate::{
    node::{LeadershipMode, PersistenceMode},
    test::{
        non_functional::*,
        utils::{self, SyncMeasurementInterval, SyncWaitParams},
        Result,
    },
    Context, ScenarioResult,
};
use jormungandr_integration_tests::common::legacy::{
    download_last_n_releases, get_jormungandr_bin,
};

use rand_chacha::ChaChaRng;
use std::path::PathBuf;

pub fn legacy_last_5th_release(context: Context<ChaChaRng>) -> Result<ScenarioResult> {
    let releases = download_last_n_releases(1);
    let last_release = releases.last().unwrap();
    let legacy_app = get_jormungandr_bin(last_release);
    test_legacy_release(context, legacy_app, "legacy_last_5th_release")
}

pub fn legacy_last_4th_release(context: Context<ChaChaRng>) -> Result<ScenarioResult> {
    let releases = download_last_n_releases(1);
    let last_release = releases.last().unwrap();
    let legacy_app = get_jormungandr_bin(last_release);
    test_legacy_release(context, legacy_app, "legacy_last_4th_release")
}

pub fn legacy_last_3rd_release(context: Context<ChaChaRng>) -> Result<ScenarioResult> {
    let releases = download_last_n_releases(1);
    let last_release = releases.last().unwrap();
    let legacy_app = get_jormungandr_bin(last_release);
    test_legacy_release(context, legacy_app, "legacy_last_3rd_release")
}

pub fn legacy_last_2nd_release(context: Context<ChaChaRng>) -> Result<ScenarioResult> {
    let releases = download_last_n_releases(1);
    let last_release = releases.last().unwrap();
    let legacy_app = get_jormungandr_bin(last_release);
    test_legacy_release(context, legacy_app, "legacy_last_2nd_release")
}

pub fn legacy_last_release(context: Context<ChaChaRng>) -> Result<ScenarioResult> {
    let releases = download_last_n_releases(1);
    let last_release = releases.last().unwrap();
    let legacy_app = get_jormungandr_bin(last_release);
    test_legacy_release(context, legacy_app, "legacy_last_release")
}

fn test_legacy_release(
    mut context: Context<ChaChaRng>,
    legacy_app: PathBuf,
    name: &str,
) -> Result<ScenarioResult> {
    let scenario_settings = prepare_scenario! {
        name,
        &mut context,
        topology [
            LEADER_3,
            LEADER_1 -> LEADER_3,LEADER_4,
            LEADER_2 -> LEADER_1,
            LEADER_4 -> LEADER_2,LEADER_3,
        ]
        blockchain {
            consensus = GenesisPraos,
            number_of_slots_per_epoch = 60,
            slot_duration = 1,
            leaders = [ LEADER_1 ],
            initials = [
                account "unassigned1" with   500_000_000,
                account "delegated1" with  2_000_000_000 delegates to LEADER_1,
                account "delegated2" with  2_000_000_000 delegates to LEADER_2,
                account "delegated3" with  2_000_000_000 delegates to LEADER_3,
                account "delegated4" with  2_000_000_000 delegates to LEADER_4
            ],
        }
    };

    let mut controller = scenario_settings.build(context)?;

    controller.monitor_nodes();
    let leader3 =
        controller.spawn_node(LEADER_3, LeadershipMode::Leader, PersistenceMode::InMemory)?;
    leader3.wait_for_bootstrap()?;
    let leader1 = controller.spawn_node_custom(
        controller
            .new_spawn_params(LEADER_1)
            .jormungandr(legacy_app),
    )?;
    leader1.wait_for_bootstrap()?;
    let leader2 =
        controller.spawn_node(LEADER_2, LeadershipMode::Leader, PersistenceMode::InMemory)?;

    leader2.wait_for_bootstrap()?;
    let leader4 =
        controller.spawn_node(LEADER_4, LeadershipMode::Leader, PersistenceMode::InMemory)?;
    leader4.wait_for_bootstrap()?;

    let mut wallet1 = controller.wallet("unassigned1")?;
    let mut wallet2 = controller.wallet("delegated1")?;

    utils::sending_transactions_to_node_sequentially(
        10,
        &mut controller,
        &mut wallet1,
        &mut wallet2,
        &leader1,
    )?;

    utils::measure_and_log_sync_time(
        vec![&leader1, &leader2, &leader3, &leader4],
        SyncWaitParams::network_size(4, 2).into(),
        name,
        SyncMeasurementInterval::Standard,
    )?;

    leader4.shutdown()?;
    leader3.shutdown()?;
    leader2.shutdown()?;
    leader1.shutdown()?;

    controller.finalize();
    Ok(ScenarioResult::passed())
}
