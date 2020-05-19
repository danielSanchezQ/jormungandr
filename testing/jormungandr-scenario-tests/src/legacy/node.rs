/// Specialized node which is supposed to be compatible with 5 last jormungandr releases
use crate::{
    legacy::LegacySettings,
    node::{MemPoolCheck, ProgressBarController, Status},
    style, Context,
};
use chain_impl_mockchain::{
    block::Block,
    fragment::{Fragment, FragmentId},
    header::HeaderId,
};
use futures::{stream::Stream, Future, StreamExt};
use indicatif::ProgressBar;
use jormungandr_integration_tests::{
    common::jormungandr::logger::JormungandrLogger,
    mock::{client::JormungandrClient, read_into},
    response_to_vec,
};
use jormungandr_lib::interfaces::{
    EnclaveLeaderId, FragmentLog, FragmentStatus, Info, PeerRecord, PeerStats,
};
pub use jormungandr_testing_utils::testing::network_builder::{
    LeadershipMode, NodeAlias, NodeBlock0, NodeSetting, PersistenceMode, Settings,
};
use rand::RngCore;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::macros::support::{Pin, Poll};
use tokio::prelude::*;
use tokio::process::{Child as Process, Command};
use yaml_rust::{Yaml, YamlLoader};

error_chain! {
    foreign_links {
        Io(std::io::Error);
        Reqwest(reqwest::Error);
        BlockFormatError(chain_core::mempack::ReadError);
        SerializationError(yaml_rust::scanner::ScanError);
    }

    errors {
        CannotCreateTemporaryDirectory {
            description("Cannot create a temporary directory")
        }

        CannotSpawnNode {
            description("Cannot spawn the node"),
        }

        InvalidHeaderId {
            description("Invalid header id"),
        }

        InvalidBlock {
            description("Invalid block"),
        }
        InvalidFragmentLogs {
            description("Fragment logs in an invalid format")
        }
        InvalidNodeStats {
            description("Node stats in an invalid format")
        }

        InvalidNetworkStats {
            description("Network stats in an invalid format")
        }

        InvalidEnclaveLeaderIds {
            description("Leaders ids in an invalid format")
        }

        InvalidPeerStats{
            description("Peer in an invalid format")
        }

        NodeStopped (status: Status) {
            description("the node is no longer running"),
        }

        NodeFailedToBootstrap (alias: String, duration: Duration, log: String) {
            description("cannot start node"),
            display("node '{}' failed to start after {} s. log: {}", alias, duration.as_secs(), log),
        }

        NodeFailedToShutdown (alias: String, message: String, log: String) {
            description("cannot shutdown node"),
            display("node '{}' failed to shutdown. Message: {}. Log: {}", alias, message, log),
        }

        FragmentNoInMemPoolLogs (alias: String, fragment_id: FragmentId, log: String) {
            description("cannot find fragment in mempool logs"),
            display("fragment '{}' not in the mempool of the node '{}'. logs: {}", fragment_id, alias, log),
        }

        FragmentIsPendingForTooLong (fragment_id: FragmentId, duration: Duration, alias: String, log: String) {
            description("fragment is pending for too long"),
            display("fragment '{}' is pending for too long ({} s). Node: {}, Logs: {}", fragment_id, duration.as_secs(), alias, log),
        }
    }
}

/// send query to a running node
#[derive(Clone)]
pub struct LegacyNodeController {
    alias: NodeAlias,
    grpc_client: JormungandrClient,
    settings: LegacySettings,
    progress_bar: ProgressBarController,
    status: Arc<Mutex<Status>>,
}

pub struct LegacyNode {
    alias: NodeAlias,

    #[allow(unused)]
    dir: PathBuf,

    process: Process,

    progress_bar: ProgressBarController,
    node_settings: LegacySettings,
    status: Arc<Mutex<Status>>,
}

const NODE_CONFIG: &str = "node_config.yaml";
const NODE_SECRET: &str = "node_secret.yaml";
const NODE_STORAGE: &str = "storage.db";

impl LegacyNodeController {
    pub fn alias(&self) -> &NodeAlias {
        &self.alias
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }

    pub fn check_running(&self) -> bool {
        self.status() == Status::Running
    }

    fn path(&self, path: &str) -> String {
        format!("{}/{}", self.base_url(), path)
    }

    pub fn address(&self) -> poldercast::Address {
        self.settings.config.p2p.public_address.clone()
    }

    fn post(&self, path: &str, body: Vec<u8>) -> Result<reqwest::blocking::Response> {
        self.progress_bar.log_info(format!("POST '{}'", path));

        let client = reqwest::blocking::Client::new();
        let res = client
            .post(&format!("{}/{}", self.base_url(), path))
            .body(body)
            .send();

        match res {
            Err(err) => {
                self.progress_bar
                    .log_err(format!("Failed to send request {}", &err));
                Err(err.into())
            }
            Ok(r) => Ok(r),
        }
    }

    fn get(&self, path: &str) -> Result<reqwest::blocking::Response> {
        self.progress_bar.log_info(format!("GET '{}'", path));

        match reqwest::blocking::get(&format!("{}/{}", self.base_url(), path)) {
            Err(err) => {
                self.progress_bar
                    .log_err(format!("Failed to send request {}", &err));
                Err(err.into())
            }
            Ok(r) => Ok(r),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/api/v0", self.settings.config.rest.listen.clone())
    }

    pub fn send_fragment(&self, fragment: Fragment) -> Result<MemPoolCheck> {
        use chain_core::property::Fragment as _;
        use chain_core::property::Serialize as _;

        let raw = fragment.serialize_as_vec().unwrap();
        let fragment_id = fragment.id();

        let response = self.post("message", raw.clone())?;
        self.progress_bar
            .log_info(format!("Fragment '{}' sent", fragment_id,));

        let res = response.error_for_status_ref();
        if let Err(err) = res {
            self.progress_bar.log_err(format!(
                "Fragment '{}' ({}) fail to send: {}",
                hex::encode(&raw),
                fragment_id,
                err,
            ));
        }

        Ok(MemPoolCheck::new(fragment_id))
    }

    pub fn log(&self, info: &str) {
        self.progress_bar.log_info(info);
    }

    pub fn tip(&self) -> Result<HeaderId> {
        let hash = self.get("tip")?.text()?;

        let hash = hash.parse().chain_err(|| ErrorKind::InvalidHeaderId)?;

        self.progress_bar.log_info(format!("tip '{}'", hash));

        Ok(hash)
    }

    pub fn blocks_to_tip(&self, from: HeaderId) -> Result<Vec<Block>> {
        let response = self.grpc_client.pull_blocks_to_tip(from);
        Ok(response_to_vec!(response))
    }

    pub fn network_stats(&self) -> Result<Vec<PeerStats>> {
        let response_text = self.get("network/stats")?.text()?;
        self.progress_bar
            .log_info(format!("network/stats: {}", response_text));

        let network_stats: Vec<PeerStats> = if response_text.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&response_text).chain_err(|| ErrorKind::InvalidNetworkStats)?
        };
        Ok(network_stats)
    }

    pub fn p2p_quarantined(&self) -> Result<Vec<PeerRecord>> {
        let response_text = self.get("network/p2p/quarantined")?.text()?;

        self.progress_bar
            .log_info(format!("network/p2p_quarantined: {}", response_text));

        let network_stats: Vec<PeerRecord> = if response_text.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&response_text).chain_err(|| ErrorKind::InvalidNetworkStats)?
        };
        Ok(network_stats)
    }

    pub fn p2p_non_public(&self) -> Result<Vec<PeerRecord>> {
        let response_text = self.get("network/p2p/non_public")?.text()?;

        self.progress_bar
            .log_info(format!("network/non_publicS: {}", response_text));

        let network_stats: Vec<PeerRecord> = if response_text.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&response_text).chain_err(|| ErrorKind::InvalidNetworkStats)?
        };
        Ok(network_stats)
    }

    pub fn p2p_available(&self) -> Result<Vec<PeerRecord>> {
        let response_text = self.get("network/p2p/available")?.text()?;

        self.progress_bar
            .log_info(format!("network/available: {}", response_text));

        let network_stats: Vec<PeerRecord> = if response_text.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&response_text).chain_err(|| ErrorKind::InvalidNetworkStats)?
        };
        Ok(network_stats)
    }

    pub fn p2p_view(&self) -> Result<Vec<Info>> {
        let response_text = self.get("network/p2p/view")?.text()?;

        self.progress_bar
            .log_info(format!("network/view: {}", response_text));

        let network_stats: Vec<Info> = if response_text.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&response_text).chain_err(|| ErrorKind::InvalidNetworkStats)?
        };
        Ok(network_stats)
    }

    pub fn all_blocks_hashes(&self) -> Result<Vec<HeaderId>> {
        let genesis_hash = self
            .genesis_block_hash()
            .expect("Cannot download genesis hash");
        self.blocks_hashes_to_tip(genesis_hash)
    }

    pub fn blocks_hashes_to_tip(&self, from: HeaderId) -> Result<Vec<HeaderId>> {
        Ok(self
            .blocks_to_tip(from)
            .unwrap()
            .iter()
            .map(|x| x.header.hash())
            .collect())
    }

    pub fn genesis_block_hash(&self) -> Result<HeaderId> {
        Ok(self.grpc_client.get_genesis_block_hash())
    }

    pub fn block(&self, header_hash: &HeaderId) -> Result<Block> {
        use chain_core::mempack::{ReadBuf, Readable as _};

        let mut resp = self.get(&format!("block/{}", header_hash))?;
        let mut bytes = Vec::new();
        resp.copy_to(&mut bytes)?;
        let block =
            Block::read(&mut ReadBuf::from(&bytes)).chain_err(|| ErrorKind::InvalidBlock)?;

        self.progress_bar.log_info(format!(
            "block{} ({}) '{}'",
            block.header.chain_length(),
            block.header.block_date(),
            header_hash,
        ));

        Ok(block)
    }

    pub fn fragment_logs(&self) -> Result<HashMap<FragmentId, FragmentLog>> {
        let logs = self.get("fragment/logs")?.text()?;

        let logs: Vec<FragmentLog> = if logs.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&logs).chain_err(|| ErrorKind::InvalidFragmentLogs)?
        };

        self.progress_bar
            .log_info(format!("fragment logs ({})", logs.len()));

        let logs = logs
            .into_iter()
            .map(|log| (log.fragment_id().clone().into_hash(), log))
            .collect();

        Ok(logs)
    }

    pub fn wait_fragment(&self, duration: Duration, check: MemPoolCheck) -> Result<FragmentStatus> {
        let max_try = 50;
        for _ in 0..max_try {
            let logs = self.fragment_logs()?;

            if let Some(log) = logs.get(&check.fragment_id()) {
                use jormungandr_lib::interfaces::FragmentStatus::*;
                let status = log.status().clone();
                match log.status() {
                    Pending => {
                        self.progress_bar.log_info(format!(
                            "Fragment '{}' is still pending",
                            check.fragment_id()
                        ));
                    }
                    Rejected { reason } => {
                        self.progress_bar.log_info(format!(
                            "Fragment '{}' rejected: {}",
                            check.fragment_id(),
                            reason
                        ));
                        return Ok(status);
                    }
                    InABlock { date, block } => {
                        self.progress_bar.log_info(format!(
                            "Fragment '{}' in block: {} ({})",
                            check.fragment_id(),
                            block,
                            date
                        ));
                        return Ok(status);
                    }
                }
            } else {
                bail!(ErrorKind::FragmentNoInMemPoolLogs(
                    self.alias().to_string(),
                    check.fragment_id().clone(),
                    self.logger().get_log_content()
                ))
            }
            std::thread::sleep(duration);
        }

        bail!(ErrorKind::FragmentIsPendingForTooLong(
            check.fragment_id().clone(),
            Duration::from_secs(duration.as_secs() * max_try),
            self.alias().to_string(),
            self.logger().get_log_content()
        ))
    }

    pub fn leaders(&self) -> Result<Vec<EnclaveLeaderId>> {
        let leaders = self.get("leaders")?.text()?;
        let leaders: Vec<EnclaveLeaderId> = if leaders.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&leaders).chain_err(|| ErrorKind::InvalidEnclaveLeaderIds)?
        };

        self.progress_bar
            .log_info(format!("leaders ids ({})", leaders.len()));

        Ok(leaders)
    }

    pub fn promote(&self) -> Result<EnclaveLeaderId> {
        let path = "leaders";
        let secrets = self.settings.secrets();
        self.progress_bar.log_info(format!("POST '{}'", &path));
        let response = reqwest::blocking::Client::new()
            .post(&self.path(path))
            .json(&secrets)
            .send()?;

        self.progress_bar
            .log_info(format!("Leader promotion for '{}' sent", self.alias()));

        let res = response.error_for_status_ref();
        if let Err(err) = res {
            self.progress_bar.log_err(format!(
                "Leader promotion for '{}' fail to sent: {}",
                self.alias(),
                err,
            ));
        }

        let leader_id: EnclaveLeaderId = response.json()?;
        Ok(leader_id)
    }

    pub fn demote(&self, leader_id: u32) -> Result<()> {
        let path = format!("leaders/{}", leader_id);
        self.progress_bar.log_info(format!("DELETE '{}'", &path));
        let response = reqwest::blocking::Client::new()
            .delete(&self.path(&path))
            .send()?;

        self.progress_bar
            .log_info(format!("Leader demote for '{}' sent", self.alias()));

        let res = response.error_for_status_ref();
        if let Err(err) = res {
            self.progress_bar.log_err(format!(
                "Leader demote for '{}' fail to sent: {}",
                self.alias(),
                err,
            ));
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<Yaml> {
        let stats = self.get("node/stats")?.text()?;
        let docs = YamlLoader::load_from_str(&stats)?;
        Ok(docs.get(0).unwrap().clone())
    }

    pub fn log_stats(&self) {
        self.progress_bar
            .log_info(format!("node stats ({:?})", self.stats()));
    }

    pub fn wait_for_bootstrap(&self) -> Result<()> {
        let max_try = 20;
        let sleep = Duration::from_secs(8);
        for _ in 0..max_try {
            let stats = self.stats();
            match stats {
                Ok(stats) => {
                    if stats["state"].as_str().unwrap() == "Running" {
                        self.log_stats();
                        return Ok(());
                    }
                }
                Err(err) => self
                    .progress_bar
                    .log_info(format!("node stats failure({:?})", err)),
            };
            std::thread::sleep(sleep);
        }
        bail!(ErrorKind::NodeFailedToBootstrap(
            self.alias().to_string(),
            Duration::from_secs(sleep.as_secs() * max_try),
            self.logger().get_log_content()
        ))
    }

    pub fn wait_for_shutdown(&self) -> Result<()> {
        let max_try = 2;
        let sleep = Duration::from_secs(2);
        for _ in 0..max_try {
            if self.stats().is_err() && self.ports_are_opened() {
                return Ok(());
            };
            std::thread::sleep(sleep);
        }
        bail!(ErrorKind::NodeFailedToShutdown(
            self.alias().to_string(),
            format!(
                "node is still up after {} s from sending shutdown request",
                sleep.as_secs()
            ),
            self.logger().get_log_content()
        ))
    }

    fn ports_are_opened(&self) -> bool {
        self.port_opened(self.settings.config.rest.listen.port())
            && self.port_opened(
                self.settings
                    .config
                    .p2p
                    .public_address
                    .to_socketaddr()
                    .unwrap()
                    .port(),
            )
    }

    fn port_opened(&self, port: u16) -> bool {
        use std::net::TcpListener;
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    pub fn is_up(&self) -> bool {
        let stats = self.stats();
        match stats {
            Ok(stats) => stats["state"].as_str().unwrap() == "Running",
            Err(_) => false,
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        let result = self.get("shutdown")?.text()?;

        if result == "" {
            self.progress_bar.log_info("shuting down");
            return self.wait_for_shutdown();
        } else {
            bail!(ErrorKind::NodeFailedToShutdown(
                self.alias().to_string(),
                result,
                self.logger().get_log_content()
            ))
        }
    }

    pub fn logger(&self) -> JormungandrLogger {
        let log_file = self
            .settings
            .config
            .log
            .clone()
            .unwrap()
            .log_file()
            .unwrap();
        JormungandrLogger::new(log_file)
    }

    pub fn log_content(&self) -> String {
        self.logger().get_log_content()
    }
}

impl LegacyNode {
    pub fn alias(&self) -> &NodeAlias {
        &self.alias
    }

    pub fn controller(&self) -> LegacyNodeController {
        let p2p_address = format!("{}", self.node_settings.config().p2p.public_address);

        LegacyNodeController {
            alias: self.alias().clone(),
            grpc_client: JormungandrClient::from_address(&p2p_address)
                .expect("cannot setup grpc client"),
            settings: self.node_settings.clone(),
            status: self.status.clone(),
            progress_bar: self.progress_bar.clone(),
        }
    }

    pub fn spawn<R: RngCore>(
        jormungandr: Arc<Command>,
        context: &Context<R>,
        progress_bar: ProgressBar,
        alias: &str,
        node_settings: &mut LegacySettings,
        block0: NodeBlock0,
        working_dir: &PathBuf,
        peristence_mode: PersistenceMode,
    ) -> Result<Self> {
        let command = jormungandr.clone();
        let dir = working_dir.join(alias);
        std::fs::DirBuilder::new().recursive(true).create(&dir)?;

        let progress_bar = ProgressBarController::new(
            progress_bar,
            format!("{}@{}", alias, node_settings.config().rest.listen),
            context.progress_bar_mode(),
        );

        let config_file = dir.join(NODE_CONFIG);
        let config_secret = dir.join(NODE_SECRET);

        if peristence_mode == PersistenceMode::Persistent {
            let path_to_storage = dir.join(NODE_STORAGE);
            node_settings.config.storage = Some(path_to_storage);
        }

        serde_yaml::to_writer(
            std::fs::File::create(&config_file)
                .chain_err(|| format!("Cannot create file {:?}", config_file))?,
            node_settings.config(),
        )
        .chain_err(|| format!("cannot write in {:?}", config_file))?;

        serde_yaml::to_writer(
            std::fs::File::create(&config_secret)
                .chain_err(|| format!("Cannot create file {:?}", config_secret))?,
            node_settings.secrets(),
        )
        .chain_err(|| format!("cannot write in {:?}", config_secret))?;

        command.args(&[
            "--config",
            &config_file.display().to_string(),
            "--log-level=warn",
        ]);

        match block0 {
            NodeBlock0::File(path) => {
                command.args(&[
                    "--genesis-block",
                    &path.display().to_string(),
                    "--secret",
                    &config_secret.display().to_string(),
                ]);
            }
            NodeBlock0::Hash(hash) => {
                command.args(&["--genesis-block-hash", &hash.to_string()]);
            }
        }

        let process = command.spawn().map_err(|_| ErrorKind::CannotSpawnNode)?;

        let node = LegacyNode {
            alias: alias.into(),

            dir,

            process,

            progress_bar,
            node_settings: node_settings.clone(),
            status: Arc::new(Mutex::new(Status::Running)),
        };

        node.progress_bar_start();

        Ok(node)
    }

    pub async fn capture_logs(&mut self) {
        let stderr = self.process.stderr.as_mut().unwrap();

        let stderr =
            tokio_util::codec::FramedRead::new(stderr, tokio_util::codec::LinesCodec::new());

        let progress_bar = self.progress_bar.clone();

        stderr.map(move |res| match res {
            Ok(line) => {
                progress_bar.log_info(&line);
            }
            Err(err) => unimplemented!("{}", err),
        });
    }

    fn progress_bar_start(&self) {
        self.progress_bar.set_style(
            indicatif::ProgressStyle::default_spinner()
                .template("{spinner:.green} {wide_msg}")
                .tick_chars(style::TICKER),
        );
        self.progress_bar.enable_steady_tick(100);
        self.progress_bar.set_message(&format!(
            "{} {} ... [{}]",
            *style::icons::jormungandr,
            style::binary.apply_to(self.alias()),
            self.node_settings.config().rest.listen,
        ));
    }

    fn progress_bar_failure(&self) {
        self.progress_bar.finish_with_message(&format!(
            "{} {} {}",
            *style::icons::jormungandr,
            style::binary.apply_to(self.alias()),
            style::error.apply_to(*style::icons::failure)
        ));
    }

    fn progress_bar_success(&self) {
        self.progress_bar.finish_with_message(&format!(
            "{} {} {}",
            *style::icons::jormungandr,
            style::binary.apply_to(self.alias()),
            style::success.apply_to(*style::icons::success)
        ));
    }

    fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status
    }
}

impl Future for LegacyNode {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context) -> Poll<Self::Output> {
        let process = self.process;
        futures::pin_mut!(process);

        match process.poll(cx) {
            Poll::Ready(Err(err)) => {
                self.progress_bar.log_err(&err);
                self.progress_bar_failure();
                self.set_status(Status::Failure);
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(status)) => {
                if status.success() {
                    self.progress_bar_success();
                } else {
                    self.progress_bar.log_err(&status);
                    self.progress_bar_failure()
                }
                self.set_status(Status::Exit(status));
                Poll::Ready(())
            }
        }
    }
}
