use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{
    CultMesh, CultMeshNodeOptions, CultMeshRudpDocumentPublishOptions, CultMeshRudpSnapshotOptions,
};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding, CultNetRudpSocketMode,
    CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions, CultNetWireContract,
    encode_cultnet_message_to_vec,
};
use odin_core::{
    EVE_PROVIDER_ADVERTISEMENT_SCHEMA, EveProviderAdvertisementRecord, EveSurfaceStateRecord,
    IdunnDaemonHealthRecord, MUNINN_HID_CONTROLLER_STATE_SCHEMA, MuninnHidControllerStateRecord,
    OdinDocuments, OdinEndpointQuery, SleipnirInputMappingRecord, discover_provider_endpoints,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

const SLEIPNIR_HID_RUDP_CONNECTION_ID: u32 = 0x6d75_0005;
const SLEIPNIR_COMMAND_ROUTE: &str = "cultmesh://odin/command/sleipnir/input_mapping";
const SLEIPNIR_INPUT_MAPPING_SCHEMA: &str = "sleipnir.input_mapping.v1";
const IDUNN_HEALTH_RUDP_CONNECTION_ID: u32 = 0x1d0d_0001;
const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
const INPUT_STALE_NEUTRAL_AFTER: Duration = Duration::from_millis(1_500);
const INPUT_STREAM_RECONNECT_AFTER: Duration = Duration::from_secs(3);
const DEFAULT_STICK_DEADZONE: f32 = 0.12;
const HID_RUDP_MAX_FRAGMENT_BYTES: u32 = 1_200;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    store_path: PathBuf,
    discovery_store_path: Option<PathBuf>,
    mapping_path: Option<PathBuf>,
    device_filter: Option<String>,
    host_id: String,
    interval_ms: u64,
    odin_cultmesh_uri: Option<String>,
    rudp_bind: Option<SocketAddr>,
    command_route: String,
    idunn_rudp_health: Option<IdunnRudpHealthOptions>,
    once: bool,
    dry_run: bool,
    trace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IdunnRudpHealthOptions {
    endpoint: SocketAddr,
    daemon_id: String,
    health_contract: String,
}

struct ActiveRudpStream {
    transport: CultNetRudpSocketTransportConnection,
    target: Option<SocketAddr>,
    last_connect_attempt_at: Option<Instant>,
    connected_logged: bool,
    last_frame_at: Option<Instant>,
    last_stale_log_at: Option<Instant>,
    last_subscription: Option<HidControllerRudpSubscription>,
    received_edges: Vec<HidButtonEdge>,
    last_subscription_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HidControllerRudpSubscription {
    device_filter: Option<String>,
    stream_id: Option<String>,
}

#[derive(Clone, Debug)]
struct SleipnirRuntimeState {
    provider_id: String,
    selected_muninn_endpoint: Option<String>,
    selected_device_filter: Option<String>,
    presentation: String,
    virtual_backend: String,
    stream_state: String,
    last_device_id: Option<String>,
    last_device_kind: Option<String>,
    last_sequence: Option<u64>,
    last_frame_age_ms: Option<u128>,
    last_input_latency: Option<SleipnirInputLatencySnapshot>,
    ignored_stream_frames: u64,
    available_devices: Vec<AvailableHidDevice>,
    axis_map: HashMap<String, AxisBinding>,
    button_map: HashMap<String, String>,
    pending_learn: Option<LearnRequest>,
    updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SleipnirInputLatencySnapshot {
    device_id: String,
    sequence: u64,
    source_to_arrival_ms: Option<i64>,
    arrival_to_buffer_ms: u128,
    buffer_to_axis_ms: u128,
    axis_to_hid_ms: u128,
    source_to_hid_ms: Option<i64>,
    total_observed_ms: u128,
    axis_summary: Vec<String>,
    emitted: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct SleipnirDesiredMapping {
    enabled: bool,
    device_filter: Option<String>,
    stream_id: Option<String>,
    presentation: String,
    axis_map: HashMap<String, AxisBinding>,
    button_map: HashMap<String, String>,
    pending_learn: Option<LearnRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LearnRequest {
    target: String,
    binding_kind: String,
    requested_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct AxisBinding {
    source: usize,
    invert: bool,
    scale: f32,
    deadzone: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AvailableHidDevice {
    device_id: String,
    device_kind: String,
    source_path: String,
    stream_id: String,
    host_id: String,
    endpoint: String,
}

#[derive(Clone, Debug, PartialEq)]
struct VirtualPadState {
    buttons: Vec<String>,
    left_x: i16,
    left_y: i16,
    left_trigger: u8,
    right_trigger: u8,
}

#[derive(Clone, Debug)]
struct TimedHidRecord {
    record: MuninnHidControllerStateRecord,
    epoch: u64,
    state_sequence: u64,
    timing: HidRecordTiming,
}

#[derive(Clone, Debug)]
struct HidLatestStateFrame {
    epoch: u64,
    state_sequence: u64,
    record: MuninnHidControllerStateRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HidButtonEdge {
    epoch: u64,
    device_id: String,
    edge_sequence: u64,
    button: String,
    pressed: bool,
}

fn decode_latest_state_frame(payload: &[u8]) -> Option<HidLatestStateFrame> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    Some(HidLatestStateFrame {
        epoch: value.get("epoch")?.as_u64()?,
        state_sequence: value.get("state_sequence")?.as_u64()?,
        record: serde_json::from_value(value.get("record")?.clone()).ok()?,
    })
}

fn decode_button_edge(payload: &[u8]) -> Option<HidButtonEdge> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    Some(HidButtonEdge {
        epoch: value.get("epoch")?.as_u64()?,
        device_id: value.get("device_id")?.as_str()?.to_string(),
        edge_sequence: value.get("edge_sequence")?.as_u64()?,
        button: value.get("button")?.as_str()?.to_string(),
        pressed: value.get("pressed")?.as_bool()?,
    })
}

#[derive(Default)]
struct HidSemanticCursor {
    epoch: u64,
    state_sequence: u64,
    edge_sequence: u64,
    pending_edges: BTreeMap<u64, HidButtonEdge>,
    retired_epochs: HashSet<u64>,
}

impl HidSemanticCursor {
    fn accept_state(&mut self, epoch: u64, sequence: u64) -> bool {
        if epoch == 0 && self.epoch != 0 { return false; }
        if self.retired_epochs.contains(&epoch) || (epoch == self.epoch && sequence <= self.state_sequence) { return false; }
        if epoch != self.epoch {
            if self.epoch != 0 { self.retired_epochs.insert(self.epoch); }
            self.epoch = epoch; self.state_sequence = 0; self.edge_sequence = 0; self.pending_edges.clear();
        }
        self.state_sequence = sequence;
        true
    }
    fn push_edge(&mut self, edge: HidButtonEdge) -> Vec<HidButtonEdge> {
        if edge.epoch != self.epoch || edge.edge_sequence <= self.edge_sequence { return Vec::new(); }
        self.pending_edges.entry(edge.edge_sequence).or_insert(edge);
        let mut ready = Vec::new();
        while let Some(edge) = self.pending_edges.remove(&(self.edge_sequence + 1)) {
            self.edge_sequence += 1;
            ready.push(edge);
        }
        ready
    }
}

#[derive(Clone, Debug)]
struct HidRecordTiming {
    frame_received_at: Instant,
    frame_received_unix_ns: i64,
    buffer_ready_at: Instant,
}

impl Default for VirtualPadState {
    fn default() -> Self {
        Self {
            buttons: Vec::new(),
            left_x: 0,
            left_y: 0,
            left_trigger: 0,
            right_trigger: 0,
        }
    }
}

trait VirtualPadBackend {
    fn update(&mut self, state: &VirtualPadState) -> Result<()>;
}

struct LoggingBackend;

impl VirtualPadBackend for LoggingBackend {
    fn update(&mut self, state: &VirtualPadState) -> Result<()> {
        println!(
            "Sleipnir dry-run virtual pad lx={} ly={} lt={} rt={} buttons=[{}]",
            state.left_x,
            state.left_y,
            state.left_trigger,
            state.right_trigger,
            state.buttons.join(",")
        );
        Ok(())
    }
}

#[cfg(windows)]
struct VigemXboxBackend {
    target: vigem_client::Xbox360Wired<vigem_client::Client>,
}

#[cfg(windows)]
impl VigemXboxBackend {
    fn connect() -> Result<Self> {
        let client = vigem_client::Client::connect().context("connecting to ViGEmBus")?;
        let mut target =
            vigem_client::Xbox360Wired::new(client, vigem_client::TargetId::XBOX360_WIRED);
        target
            .plugin()
            .context("plugging in virtual Xbox 360 controller")?;
        target
            .wait_ready()
            .context("waiting for virtual Xbox 360 controller readiness")?;
        Ok(Self { target })
    }
}

#[cfg(windows)]
impl VirtualPadBackend for VigemXboxBackend {
    fn update(&mut self, state: &VirtualPadState) -> Result<()> {
        let mut raw_buttons = 0u16;
        for button in &state.buttons {
            raw_buttons |= xbutton_mask(button);
        }
        let gamepad = vigem_client::XGamepad {
            buttons: vigem_client::XButtons::from(raw_buttons),
            left_trigger: state.left_trigger,
            right_trigger: state.right_trigger,
            thumb_lx: state.left_x,
            thumb_ly: state.left_y,
            thumb_rx: 0,
            thumb_ry: 0,
            ..Default::default()
        };
        self.target
            .update(&gamepad)
            .context("updating virtual Xbox 360 controller")
    }
}

#[cfg(windows)]
fn xbutton_mask(button: &str) -> u16 {
    match button {
        "up" => vigem_client::XButtons::UP,
        "down" => vigem_client::XButtons::DOWN,
        "left" => vigem_client::XButtons::LEFT,
        "right" => vigem_client::XButtons::RIGHT,
        "start" => vigem_client::XButtons::START,
        "select" | "back" => vigem_client::XButtons::BACK,
        "l3" => vigem_client::XButtons::LTHUMB,
        "r3" => vigem_client::XButtons::RTHUMB,
        "l1" => vigem_client::XButtons::LB,
        "r1" => vigem_client::XButtons::RB,
        "ps" | "guide" => vigem_client::XButtons::GUIDE,
        "cross" | "a" => vigem_client::XButtons::A,
        "circle" | "o" | "b" => vigem_client::XButtons::B,
        "square" | "x" => vigem_client::XButtons::X,
        "triangle" | "y" => vigem_client::XButtons::Y,
        _ => 0,
    }
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;
    let mut node = CultMesh::create_node(
        &options.store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "sleipnir-daemon".to_string(),
            pull_on_start: true,
        },
    )
    .with_context(|| format!("opening CultMesh store {}", options.store_path.display()))?;
    let mut discovery_node =
        if let Some(discovery_store_path) = options.discovery_store_path.as_ref() {
            Some(
                CultMesh::create_node(
                    discovery_store_path,
                    OdinDocuments,
                    CultMeshNodeOptions {
                        runtime_id: "sleipnir-discovery".to_string(),
                        pull_on_start: true,
                    },
                )
                .with_context(|| {
                    format!(
                        "opening Sleipnir discovery CultMesh store {}",
                        discovery_store_path.display()
                    )
                })?,
            )
        } else {
            None
        };
    pull_odin_catalog_snapshot(discovery_node.as_mut().unwrap_or(&mut node), &options);

    let mut desired_mapping =
        read_desired_mapping_from_sources(&mut node, discovery_node.as_mut(), &options);
    let mut backend: Box<dyn VirtualPadBackend> = if options.dry_run {
        Box::new(LoggingBackend)
    } else {
        create_backend()?
    };
    let mut current_device_filter = effective_device_filter(&options, &desired_mapping);
    let discovered_endpoint = if desired_mapping.enabled {
        effective_muninn_hid_endpoint(
            discovery_node.as_ref().unwrap_or(&node),
            current_device_filter.as_deref(),
            desired_mapping.stream_id.as_deref(),
            options.trace,
        )
    } else {
        None
    };
    if options.trace {
        if !desired_mapping.enabled {
            eprintln!("Sleipnir input consumption is disabled; publishing control surface only");
        } else {
            match discovered_endpoint.as_deref() {
                Some(endpoint) => eprintln!("Sleipnir selected Muninn HID endpoint {endpoint}"),
                None => eprintln!("Sleipnir found no Odin-discovered Muninn HID endpoint; waiting"),
            }
        }
    }
    let mut rudp_stream =
        create_rudp_stream(options.rudp_bind, discovered_endpoint, options.trace)?;
    let mut last_rudp_discovery_attempt_at = Instant::now();
    let initial_state = {
        let discovery_ref = discovery_node.as_ref().unwrap_or(&node);
        SleipnirRuntimeState::from_runtime(
            &options,
            current_device_filter.as_deref(),
            discovery_ref,
            rudp_stream.as_ref(),
            None,
            0,
            &desired_mapping,
            None,
        )
    };
    publish_sleipnir_runtime_surface(&mut node, &options, &initial_state, true)?;
    let mut last_idunn_health_attempt_at = Instant::now() - Duration::from_secs(60);
    publish_idunn_health_if_configured(&options, &initial_state, &mut last_idunn_health_attempt_at);

    let mut semantic_cursor = HidSemanticCursor::default();
    let mut ignored_stream_frames = 0u64;
    let mut last_surface_publish_at = Instant::now() - Duration::from_secs(60);
    let mut last_odin_catalog_pull_at = Instant::now() - Duration::from_secs(60);
    let mut recent_applied_record: Option<MuninnHidControllerStateRecord> = None;
    let mut recent_input_latency: Option<SleipnirInputLatencySnapshot> = None;
    let mut output_is_neutral = true;
    let mut last_output_frame_at: Option<Instant> = None;
    loop {
        let input_stream_needs_discovery = rudp_stream
            .as_ref()
            .is_none_or(|stream| stream.target.is_none());
        if input_stream_needs_discovery
            && last_odin_catalog_pull_at.elapsed() >= Duration::from_secs(2)
        {
            pull_odin_catalog_snapshot(discovery_node.as_mut().unwrap_or(&mut node), &options);
            last_odin_catalog_pull_at = Instant::now();
        }
        let next_desired_mapping =
            read_desired_mapping_from_sources(&mut node, discovery_node.as_mut(), &options);
        if next_desired_mapping != desired_mapping {
            desired_mapping = next_desired_mapping;
            current_device_filter = effective_device_filter(&options, &desired_mapping);
            let rediscovered_endpoint = if desired_mapping.enabled {
                effective_muninn_hid_endpoint(
                    discovery_node.as_ref().unwrap_or(&node),
                    current_device_filter.as_deref(),
                    desired_mapping.stream_id.as_deref(),
                    options.trace,
                )
            } else {
                None
            };
            if options.trace {
                eprintln!(
                    "Sleipnir mapping changed device={:?} discovered_muninn_endpoint={:?}",
                    current_device_filter, rediscovered_endpoint
                );
            }
            backend.update(&VirtualPadState::default())?;
            output_is_neutral = true;
            last_output_frame_at = Some(Instant::now());
            drop(rudp_stream.take());
            rudp_stream =
                create_rudp_stream(options.rudp_bind, rediscovered_endpoint, options.trace)?;
            semantic_cursor = HidSemanticCursor::default();
        }
        let mut last_applied_record: Option<MuninnHidControllerStateRecord> = None;
        if let Some(stream) = rudp_stream.as_mut() {
            if ensure_rudp_connected(stream, options.trace)? {
                if !output_is_neutral {
                    backend.update(&VirtualPadState::default())?;
                    output_is_neutral = true;
                    last_output_frame_at = Some(Instant::now());
                    semantic_cursor = HidSemanticCursor::default();
                    if options.trace {
                        eprintln!(
                            "Sleipnir neutralized virtual pad before recreating stale Muninn HID RUDP connection for filter={:?}",
                            current_device_filter
                        );
                    }
                }
                let rediscovered_endpoint = if desired_mapping.enabled {
                    effective_muninn_hid_endpoint(
                        discovery_node.as_ref().unwrap_or(&node),
                        current_device_filter.as_deref(),
                        desired_mapping.stream_id.as_deref(),
                        options.trace,
                    )
                } else {
                    None
                };
                drop(rudp_stream.take());
                rudp_stream =
                    create_rudp_stream(options.rudp_bind, rediscovered_endpoint, options.trace)?;
                last_rudp_discovery_attempt_at = Instant::now();
            } else {
                send_hid_subscription_if_due(
                    stream,
                    current_device_filter.clone(),
                    desired_mapping.stream_id.clone(),
                    options.trace,
                )?;
                let records = receive_rudp_records(stream, options.trace)?;
                for timed_record in records {
                    let epoch = timed_record.epoch;
                    let state_sequence = timed_record.state_sequence;
                    let record = timed_record.record;
                    let timing = timed_record.timing;
                    if record_matches_filter(&record, current_device_filter.as_deref()) {
                        stream.last_frame_at = Some(Instant::now());
                        stream.last_stale_log_at = None;
                        if desired_mapping.pending_learn.is_some() {
                            if let Some(updated_mapping) =
                                apply_pending_learn(&mut node, &options, &desired_mapping, &record)?
                            {
                                desired_mapping = updated_mapping;
                            }
                        }
                        if semantic_cursor.accept_state(epoch, state_sequence) || options.once {
                            let state = map_record_to_virtual_pad(&record, &desired_mapping);
                            let axis_classified_at = Instant::now();
                            let output_frame_at = Instant::now();
                            if options.trace {
                                let source_age_ms = unix_nanos_i64()
                                    .saturating_sub(record.source_timestamp_ns)
                                    / 1_000_000;
                                eprintln!(
                                    "Sleipnir applying fast HID frame device={} seq={} source_age_ms={} lx={} ly={} lt={} rt={} buttons=[{}]",
                                    record.device_id,
                                    record.sequence,
                                    source_age_ms,
                                    state.left_x,
                                    state.left_y,
                                    state.left_trigger,
                                    state.right_trigger,
                                    state.buttons.join(",")
                                );
                            }
                            match backend.update(&state) {
                                Ok(()) => {
                                    let hid_emitted_at = Instant::now();
                                    recent_input_latency = Some(input_latency_snapshot(
                                        &record,
                                        &timing,
                                        axis_classified_at,
                                        hid_emitted_at,
                                        &desired_mapping,
                                        true,
                                    ));
                                    output_is_neutral = state == VirtualPadState::default();
                                    last_output_frame_at = Some(output_frame_at);
                                    last_applied_record = Some(record);
                                }
                                Err(error) => {
                                    eprintln!(
                                        "Sleipnir virtual pad update failed for device={} seq={}: {error:#}",
                                        record.device_id, record.sequence
                                    );
                                    recent_input_latency = Some(input_latency_snapshot(
                                        &record,
                                        &timing,
                                        axis_classified_at,
                                        Instant::now(),
                                        &desired_mapping,
                                        false,
                                    ));
                                    last_applied_record = Some(record);
                                }
                            }
                        }
                    } else if options.trace {
                        ignored_stream_frames = ignored_stream_frames.saturating_add(1);
                        if ignored_stream_frames <= 5 || ignored_stream_frames % 120 == 0 {
                            eprintln!(
                                "Sleipnir ignored fast HID frame device={} kind={} stream={} filter={:?}",
                                record.device_id,
                                record.device_kind,
                                record.stream_id,
                                options.device_filter
                            );
                        }
                    }
                }
                let edges = std::mem::take(&mut stream.received_edges);
                for edge in edges {
                    let edge_epoch = edge.epoch;
                    let edge_sequence = edge.edge_sequence;
                    let edge_device = edge.device_id.clone();
                    for applied in semantic_cursor.push_edge(edge) {
                        let Some(record) = last_applied_record.as_mut().or(recent_applied_record.as_mut()) else { continue; };
                        if applied.pressed {
                            if !record.buttons.contains(&applied.button) { record.buttons.push(applied.button.clone()); }
                        } else {
                            record.buttons.retain(|button| button != &applied.button);
                        }
                        let state = map_record_to_virtual_pad(record, &desired_mapping);
                        backend.update(&state)?;
                        output_is_neutral = state == VirtualPadState::default();
                        last_output_frame_at = Some(Instant::now());
                    }
                    if edge_epoch == semantic_cursor.epoch && edge_sequence <= semantic_cursor.edge_sequence {
                        let ack = serde_json::json!({"epoch": edge_epoch, "device_id": edge_device, "edge_sequence": semantic_cursor.edge_sequence});
                        stream.transport.send("hid.edge.ack", serde_json::to_vec(&ack)?)?;
                    }
                }
                if desired_mapping.enabled
                    && !output_is_neutral
                    && last_output_frame_at
                        .is_none_or(|last_frame| last_frame.elapsed() >= INPUT_STALE_NEUTRAL_AFTER)
                {
                    let stale_for = last_output_frame_at.map(|last_frame| last_frame.elapsed());
                    backend.update(&VirtualPadState::default())?;
                    output_is_neutral = true;
                    last_output_frame_at = Some(Instant::now());
                    if options.trace {
                        eprintln!(
                            "Sleipnir neutralized virtual pad after {:?} without a new selected input sequence for filter={:?}",
                            stale_for, current_device_filter
                        );
                    }
                }
            }
        } else if last_rudp_discovery_attempt_at.elapsed() >= Duration::from_secs(2) {
            let rediscovered_endpoint = if desired_mapping.enabled {
                effective_muninn_hid_endpoint(
                    discovery_node.as_ref().unwrap_or(&node),
                    current_device_filter.as_deref(),
                    desired_mapping.stream_id.as_deref(),
                    options.trace,
                )
            } else {
                None
            };
            if rediscovered_endpoint.is_some() {
                drop(rudp_stream.take());
                rudp_stream =
                    create_rudp_stream(options.rudp_bind, rediscovered_endpoint, options.trace)?;
                let runtime_state = {
                    let discovery_ref = discovery_node.as_ref().unwrap_or(&node);
                    SleipnirRuntimeState::from_runtime(
                        &options,
                        current_device_filter.as_deref(),
                        discovery_ref,
                        rudp_stream.as_ref(),
                        recent_applied_record.as_ref(),
                        ignored_stream_frames,
                        &desired_mapping,
                        recent_input_latency.as_ref(),
                    )
                };
                publish_sleipnir_runtime_surface(&mut node, &options, &runtime_state, true)?;
                publish_idunn_health_if_configured(
                    &options,
                    &runtime_state,
                    &mut last_idunn_health_attempt_at,
                );
            } else if options.trace && desired_mapping.enabled {
                eprintln!("Sleipnir still has no Odin-discovered Muninn HID endpoint");
            }
            last_rudp_discovery_attempt_at = Instant::now();
        }
        let surface_age = last_surface_publish_at.elapsed();
        let has_new_applied_record = last_applied_record.is_some();
        let missing_input_target = rudp_stream
            .as_ref()
            .is_none_or(|stream| stream.target.is_none());
        if let Some(record) = last_applied_record.as_ref() {
            recent_applied_record = Some(record.clone());
        }
        if (has_new_applied_record && surface_age >= Duration::from_secs(1))
            || (missing_input_target && surface_age >= Duration::from_secs(5))
        {
            let runtime_state = {
                let discovery_ref = discovery_node.as_ref().unwrap_or(&node);
                SleipnirRuntimeState::from_runtime(
                    &options,
                    current_device_filter.as_deref(),
                    discovery_ref,
                    rudp_stream.as_ref(),
                    recent_applied_record.as_ref(),
                    ignored_stream_frames,
                    &desired_mapping,
                    recent_input_latency.as_ref(),
                )
            };
            publish_sleipnir_runtime_surface(
                &mut node,
                &options,
                &runtime_state,
                has_new_applied_record,
            )?;
            publish_idunn_health_if_configured(
                &options,
                &runtime_state,
                &mut last_idunn_health_attempt_at,
            );
            last_surface_publish_at = Instant::now();
        }
        if options.once {
            break;
        }
        thread::sleep(Duration::from_millis(options.interval_ms.max(1)));
    }
    Ok(())
}

impl SleipnirRuntimeState {
    fn from_runtime(
        options: &Options,
        selected_device_filter: Option<&str>,
        discovery_node: &cultmesh_rs::CultMeshNode,
        stream: Option<&ActiveRudpStream>,
        last_record: Option<&MuninnHidControllerStateRecord>,
        ignored_stream_frames: u64,
        desired_mapping: &SleipnirDesiredMapping,
        last_input_latency: Option<&SleipnirInputLatencySnapshot>,
    ) -> Self {
        let has_recent_frame = stream
            .and_then(|stream| stream.last_frame_at)
            .is_some_and(|last_frame| last_frame.elapsed() < Duration::from_secs(3));
        let stream_state = match stream {
            _ if !desired_mapping.enabled => "idle",
            Some(_) if has_recent_frame => "connected",
            Some(stream) if stream.transport.connected() => "connected",
            Some(stream) if stream.target.is_some() => "connecting",
            Some(_) => "listening",
            None => "discovering",
        }
        .to_string();
        let selected_muninn_endpoint = stream
            .and_then(|stream| stream.target)
            .map(|target| target.to_string());
        let mut available_devices = discover_available_hid_devices(discovery_node);
        if let (Some(record), Some(endpoint)) = (last_record, selected_muninn_endpoint.as_deref()) {
            upsert_available_hid_device(
                &mut available_devices,
                AvailableHidDevice {
                    device_id: record.device_id.clone(),
                    device_kind: record.device_kind.clone(),
                    source_path: record.source_path.clone(),
                    stream_id: record.stream_id.clone(),
                    host_id: record.host_id.clone(),
                    endpoint: endpoint.to_string(),
                },
            );
        }
        Self {
            provider_id: sleipnir_provider_id(options),
            selected_muninn_endpoint,
            selected_device_filter: selected_device_filter.map(ToString::to_string),
            presentation: default_presentation(),
            virtual_backend: if options.dry_run {
                "logging.dry-run"
            } else if cfg!(windows) {
                "vigem.xbox360"
            } else {
                "unavailable"
            }
            .to_string(),
            stream_state,
            last_device_id: last_record.map(|record| record.device_id.clone()),
            last_device_kind: last_record.map(|record| record.device_kind.clone()),
            last_sequence: last_record.map(|record| record.sequence),
            last_frame_age_ms: stream
                .and_then(|stream| stream.last_frame_at)
                .map(|last_frame| last_frame.elapsed().as_millis()),
            last_input_latency: last_input_latency.cloned(),
            ignored_stream_frames,
            available_devices,
            axis_map: desired_mapping.axis_map.clone(),
            button_map: desired_mapping.button_map.clone(),
            pending_learn: desired_mapping.pending_learn.clone(),
            updated_at: timestamp(),
        }
    }
}

fn publish_sleipnir_runtime_surface(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    state: &SleipnirRuntimeState,
    publish_remote: bool,
) -> Result<()> {
    let mut advertisement = EveProviderAdvertisementRecord {
        value: serde_json::json!({
            "schema": "gamecult.eve.provider_advertisement.v1",
            "providerId": state.provider_id,
            "daemonId": sleipnir_daemon_id(options),
            "title": sleipnir_provider_title(options),
            "description": "Sleipnir mirrors Muninn-discovered input streams into a local virtual HID backend.",
            "canonicalService": "asgard.sleipnir",
            "locatedService": format!("asgard.{}.sleipnir", options.host_id),
            "verseId": format!("{}.local", options.host_id),
            "cultMeshAddress": format!("asgard.{}.sleipnir/input-mirror", options.host_id),
            "status": state.stream_state,
            "mode": "daemon-live",
            "updatedAt": state.updated_at,
            "stateStore": options.store_path.display().to_string(),
            "surfaceId": state.provider_id,
            "capabilities": [
                "muninn.input.discovery",
                "cultmesh.input.stream.consumer",
                "virtual-hid.xinput"
            ],
            "endpoints": [
                {
                    "transport": "cultcache-store",
                    "address": options.store_path.display().to_string()
                }
            ],
            "routes": [
                {
                    "transport": "cultcache-store",
                    "address": options.store_path.display().to_string()
                }
            ]
        }),
    };
    if let Some(routes) = advertisement
        .value
        .get_mut("routes")
        .and_then(|value| value.as_array_mut())
    {
        routes.push(serde_json::json!({
            "transport": "cultnet.transport.rudp.v0",
            "address": options.command_route,
            "resolver": "odin-cultmesh",
            "schema": SLEIPNIR_INPUT_MAPPING_SCHEMA,
            "role": "sleipnir.input_mapping.command",
            "channel": "schema"
        }));
    }
    let surface = EveSurfaceStateRecord {
        provider_id: state.provider_id.clone(),
        title: sleipnir_provider_title(options),
        version: unix_millis_i64(),
        updated_at: state.updated_at.clone(),
        surface: sleipnir_surface_document(state),
    };
    node.put(&state.provider_id, &advertisement)?;
    node.put(&state.provider_id, &surface)?;
    if publish_remote && let Some(target) = resolve_odin_cultmesh_uri(options) {
        if let Err(error) = node.publish_document_to_rudp_catalog(
            &state.provider_id,
            &advertisement,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: sleipnir_daemon_id(options),
                source_role: Some("sleipnir.input-mirror-provider".to_string()),
                tags: vec!["sleipnir".to_string(), "odin-verse-discovery".to_string()],
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        ) {
            if options.trace {
                eprintln!("Sleipnir could not publish provider advertisement to Odin: {error:#}");
            }
        }
        if let Err(error) = node.publish_document_to_rudp_catalog(
            &state.provider_id,
            &surface,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: sleipnir_daemon_id(options),
                source_role: Some("sleipnir.input-mirror-surface".to_string()),
                tags: vec!["sleipnir".to_string(), "eve-surface".to_string()],
                flush_timeout: Duration::from_millis(300),
                resend_delay_ms: 15,
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        ) {
            if options.trace {
                eprintln!("Sleipnir could not publish surface state to Odin: {error:#}");
            }
        }
    }
    Ok(())
}

fn publish_idunn_health_if_configured(
    options: &Options,
    state: &SleipnirRuntimeState,
    last_attempt_at: &mut Instant,
) {
    let Some(idunn) = options.idunn_rudp_health.as_ref() else {
        return;
    };
    if last_attempt_at.elapsed() < Duration::from_secs(5) {
        return;
    }
    *last_attempt_at = Instant::now();
    let health_state = if state.stream_state == "connected" || state.stream_state == "idle" {
        "active"
    } else {
        "degraded"
    };
    let detail = format!(
        "Sleipnir {}: endpoint={} device={} sequence={} age_ms={}",
        state.stream_state,
        state
            .selected_muninn_endpoint
            .as_deref()
            .unwrap_or("discovering"),
        state
            .selected_device_filter
            .as_deref()
            .unwrap_or("unselected"),
        state
            .last_sequence
            .map(|sequence| sequence.to_string())
            .unwrap_or_else(|| "none".to_string()),
        state
            .last_frame_age_ms
            .map(|age| age.to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    if let Err(error) = publish_idunn_rudp_health(idunn, health_state, &detail, &state.updated_at) {
        if options.trace {
            eprintln!(
                "Sleipnir could not publish Idunn RUDP health for {} at {}: {error:#}",
                idunn.daemon_id, state.updated_at
            );
        }
    }
}

fn publish_idunn_rudp_health(
    options: &IdunnRudpHealthOptions,
    state: &str,
    detail: &str,
    observed_at: &str,
) -> Result<()> {
    let health = IdunnDaemonHealthRecord {
        daemon_id: options.daemon_id.clone(),
        state: state.to_string(),
        detail: detail.to_string(),
        observed_at: observed_at.to_string(),
        health_contract: options.health_contract.clone(),
        publication_source: "daemon-published".to_string(),
        transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
    };
    let message = CultNetMessage::DocumentPutRaw {
        message_id: format!(
            "sleipnir-health:{}:{}",
            options.daemon_id,
            observed_at.replace(':', "-")
        ),
        document: CultNetRawDocumentRecord {
            schema_id: "idunn.daemon_health".to_string(),
            record_key: options.daemon_id.clone(),
            stored_at: observed_at.to_string(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload: rmp_serde::to_vec(&health).context("encoding Idunn daemon health")?,
            source_runtime_id: Some("sleipnir-daemon".to_string()),
            source_agent_id: None,
            source_role: Some("daemon-health-publisher".to_string()),
            tags: Some(vec![CULTNET_RUDP_PROTOCOL_ID.to_string()]),
        },
    };
    let bind_address = if options.endpoint.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Sleipnir Idunn RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut transport =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
            "sleipnir-daemon",
            socket,
            options.endpoint,
            IDUNN_HEALTH_RUDP_CONNECTION_ID,
        ))?;
    transport.connect(Vec::new())?;
    let deadline = Instant::now() + Duration::from_millis(300);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Sleipnir RUDP sender to {}",
                options.endpoint
            ));
        }
    }
    let payload = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
        .context("encoding Idunn health CultNet message")?;
    transport
        .send("schema", payload)
        .with_context(|| format!("sending Sleipnir Idunn health to {}", options.endpoint))?;
    Ok(())
}

fn resolve_odin_cultmesh_uri(options: &Options) -> Option<SocketAddr> {
    let uri = options.odin_cultmesh_uri.as_deref()?.trim();
    if uri.is_empty() {
        return None;
    }
    match CultMesh::resolve_rudp_endpoint(uri) {
        Ok(target) => Some(target),
        Err(error) => {
            if options.trace {
                eprintln!("Sleipnir could not resolve Odin CultMesh URI {uri}: {error:#}");
            }
            None
        }
    }
}

fn pull_odin_catalog_snapshot(node: &mut cultmesh_rs::CultMeshNode, options: &Options) {
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return;
    };
    let hid_result = node.pull_rudp_catalog_snapshot(CultMeshRudpSnapshotOptions {
        target,
        runtime_id: format!("sleipnir-{}-catalog-client", options.host_id),
        schema_ids: Some(vec![
            MUNINN_HID_CONTROLLER_STATE_SCHEMA.to_string(),
            SLEIPNIR_INPUT_MAPPING_SCHEMA.to_string(),
        ]),
        connect_timeout: Duration::from_millis(150),
        response_timeout: Duration::from_millis(150),
        resend_delay_ms: 15,
        ..CultMeshRudpSnapshotOptions::default()
    });
    match hid_result {
        Ok(count) if options.trace => {
            eprintln!("Sleipnir pulled {count} Odin HID/mapping discovery records from {target}");
        }
        Ok(_) => {}
        Err(error) if options.trace => {
            eprintln!("Sleipnir Odin HID/mapping snapshot pull failed from {target}: {error:#}");
        }
        Err(_) => return,
    }

    let provider_keys = muninn_provider_keys_for_discovered_hid_hosts(node);
    if provider_keys.is_empty() {
        return;
    }
    let provider_result = node.pull_rudp_catalog_snapshot(CultMeshRudpSnapshotOptions {
        target,
        runtime_id: format!("sleipnir-{}-provider-catalog-client", options.host_id),
        schema_ids: Some(vec![EVE_PROVIDER_ADVERTISEMENT_SCHEMA.to_string()]),
        record_keys: Some(provider_keys),
        connect_timeout: Duration::from_millis(150),
        response_timeout: Duration::from_millis(150),
        resend_delay_ms: 15,
        ..CultMeshRudpSnapshotOptions::default()
    });
    match provider_result {
        Ok(count) if options.trace => {
            eprintln!("Sleipnir pulled {count} Odin Muninn provider records from {target}");
        }
        Ok(_) => {}
        Err(error) if options.trace => {
            eprintln!(
                "Sleipnir Odin Muninn provider snapshot pull failed from {target}: {error:#}"
            );
        }
        Err(_) => {}
    }
}

fn muninn_provider_keys_for_discovered_hid_hosts(node: &cultmesh_rs::CultMeshNode) -> Vec<String> {
    let mut keys = node
        .cache()
        .get_all::<MuninnHidControllerStateRecord>()
        .ok()
        .into_iter()
        .flatten()
        .map(|record| format!("muninn.telemetry.{}", record.host_id))
        .collect::<Vec<_>>();
    for envelope in node.cache().snapshot() {
        if envelope.schema_id.as_deref() != Some(MUNINN_HID_CONTROLLER_STATE_SCHEMA)
            && envelope.r#type != "muninn.hid_controller_state"
        {
            continue;
        }
        let Ok(value) = rmp_serde::from_slice::<serde_json::Value>(&envelope.payload) else {
            continue;
        };
        if let Some(host_id) = hid_state_host_id_from_value(&value) {
            keys.push(format!("muninn.telemetry.{host_id}"));
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn hid_state_host_id_from_value(value: &serde_json::Value) -> Option<String> {
    let payload = value
        .as_array()
        .and_then(|items| {
            (items.len() == 1)
                .then(|| items.first())
                .flatten()
                .filter(|item| item.is_object() || item.is_array())
        })
        .unwrap_or(value);
    if let Some(items) = payload.as_array() {
        return items.get(1)?.as_str().map(ToString::to_string);
    }
    string_field(payload, &["host_id", "hostId"])
}

fn sleipnir_surface_document(state: &SleipnirRuntimeState) -> serde_json::Value {
    let preset_cards = remap_preset_cards(state);
    let binding_map = xbox_binding_map(state);
    let device_cards = state
        .available_devices
        .iter()
        .map(|device| {
            let selected = state.selected_device_filter.as_deref() == Some(device.device_id.as_str())
                || state.selected_device_filter.as_deref() == Some(device.device_kind.as_str())
                || state.selected_device_filter.as_deref() == Some(device.stream_id.as_str());
            serde_json::json!({
                "id": format!("{}.device.{}", state.provider_id, stable_id(&device.device_id)),
                "kind": "card",
                "props": {
                    "title": format!("{} {}", if selected { "selected" } else { "available" }, device.device_id),
                    "commandId": "sleipnir.map-hid-device",
                    "action": {
                        "type": "sleipnir.map-hid-device",
                        "providerId": state.provider_id,
                        "deviceFilter": device.device_id,
                        "streamId": device.stream_id
                    }
                },
                "children": [
                    text_element(format!("{}.device.{}.kind", state.provider_id, stable_id(&device.device_id)), format!("kind: {}", device.device_kind)),
                    text_element(format!("{}.device.{}.host", state.provider_id, stable_id(&device.device_id)), format!("host: {}", device.host_id)),
                    text_element(format!("{}.device.{}.endpoint", state.provider_id, stable_id(&device.device_id)), format!("endpoint: {}", device.endpoint)),
                    text_element(format!("{}.device.{}.path", state.provider_id, stable_id(&device.device_id)), format!("source: {}", device.source_path))
                ]
            })
        })
        .collect::<Vec<_>>();
    let mut runtime_children = vec![
        serde_json::json!({
            "id": format!("{}.disable", state.provider_id),
            "kind": "card",
            "props": {
                "title": "Stop consuming input",
                "commandId": "sleipnir.disable-input",
                "action": {
                    "type": "sleipnir.disable-input",
                    "providerId": state.provider_id
                }
            },
            "children": []
        }),
        text_element(
            format!("{}.state", state.provider_id),
            format!("state: {}", state.stream_state),
        ),
        text_element(
            format!("{}.backend", state.provider_id),
            format!("virtual backend: {}", state.virtual_backend),
        ),
        text_element(
            format!("{}.presentation", state.provider_id),
            format!("presentation: {}", state.presentation),
        ),
        text_element(
            format!("{}.muninn", state.provider_id),
            format!(
                "muninn endpoint: {}",
                state
                    .selected_muninn_endpoint
                    .as_deref()
                    .unwrap_or("discovering")
            ),
        ),
        text_element(
            format!("{}.filter", state.provider_id),
            format!(
                "device filter: {}",
                state.selected_device_filter.as_deref().unwrap_or("any")
            ),
        ),
        text_element(
            format!("{}.updated", state.provider_id),
            format!("updated: {}", state.updated_at),
        ),
    ];
    runtime_children.push(serde_json::json!({
        "id": format!("{}.devices-inline", state.provider_id),
        "kind": "pane",
        "props": { "title": "Available HID Devices", "density": "compact" },
        "children": if device_cards.is_empty() {
            vec![text_element(
                format!("{}.devices.empty", state.provider_id),
                "no Muninn HID devices discovered".to_string(),
            )]
        } else {
            device_cards
        }
    }));
    serde_json::json!({
        "schema": "gamecult.eve.surface.v1",
        "id": format!("{}.surface", state.provider_id),
        "title": "Sleipnir Input Mirror",
        "root": {
            "id": format!("{}.root", state.provider_id),
            "kind": "dashboard",
            "props": {
                "title": "Sleipnir Input Mirror",
                "summary": format!("{} via {}", state.stream_state, state.virtual_backend)
            },
            "children": [
                {
                    "id": format!("{}.runtime", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Runtime" },
                    "children": runtime_children
                },
                {
                    "id": format!("{}.input", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Input" },
                    "children": [
                        text_element(format!("{}.device", state.provider_id), format!("device: {}", state.last_device_id.as_deref().unwrap_or("none"))),
                        text_element(format!("{}.kind", state.provider_id), format!("kind: {}", state.last_device_kind.as_deref().unwrap_or("unknown"))),
                        text_element(format!("{}.sequence", state.provider_id), format!("sequence: {}", state.last_sequence.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string()))),
                        text_element(format!("{}.age", state.provider_id), format!("last frame age ms: {}", state.last_frame_age_ms.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string()))),
                        text_element(format!("{}.ignored", state.provider_id), format!("ignored stream frames: {}", state.ignored_stream_frames))
                    ]
                },
                {
                    "id": format!("{}.latency", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Input Latency" },
                    "children": input_latency_elements(state)
                },
                {
                    "id": format!("{}.axis", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Axis Classification" },
                    "children": [
                        text_element(format!("{}.axis.summary", state.provider_id), format!("mapped axes: {}", state.last_input_latency.as_ref().map(|trace| trace.axis_summary.join(", ")).filter(|text| !text.is_empty()).unwrap_or_else(|| "none".to_string())))
                    ]
                },
                {
                    "id": format!("{}.remap", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Remapping Presets" },
                    "children": preset_cards
                },
                {
                    "id": format!("{}.binding-map-pane", state.provider_id),
                    "kind": "pane",
                    "props": { "title": "Xbox Controller Mapping", "span": "wide" },
                    "children": [binding_map]
                }
            ]
        },
        "assets": []
    })
}

fn input_latency_elements(state: &SleipnirRuntimeState) -> Vec<serde_json::Value> {
    let Some(trace) = state.last_input_latency.as_ref() else {
        return vec![text_element(
            format!("{}.latency.empty", state.provider_id),
            "no emitted input trace yet".to_string(),
        )];
    };
    vec![
        text_element(
            format!("{}.latency.identity", state.provider_id),
            format!(
                "device: {} seq={} emitted={}",
                trace.device_id, trace.sequence, trace.emitted
            ),
        ),
        text_element(
            format!("{}.latency.source-arrival", state.provider_id),
            format!(
                "signal arrival: {}",
                trace
                    .source_to_arrival_ms
                    .map(|value| format!("source to arrival {} ms", value))
                    .unwrap_or_else(|| "source timestamp unavailable".to_string())
            ),
        ),
        text_element(
            format!("{}.latency.buffer", state.provider_id),
            format!("arrival to buffer: {} ms", trace.arrival_to_buffer_ms),
        ),
        text_element(
            format!("{}.latency.axis", state.provider_id),
            format!(
                "buffer to axis classification: {} ms",
                trace.buffer_to_axis_ms
            ),
        ),
        text_element(
            format!("{}.latency.hid", state.provider_id),
            format!("axis to HID emission: {} ms", trace.axis_to_hid_ms),
        ),
        text_element(
            format!("{}.latency.total", state.provider_id),
            format!(
                "observed mirror total: {} ms{}",
                trace.total_observed_ms,
                trace
                    .source_to_hid_ms
                    .map(|value| format!(" / source to HID {} ms", value))
                    .unwrap_or_default()
            ),
        ),
    ]
}

fn remap_preset_cards(state: &SleipnirRuntimeState) -> Vec<serde_json::Value> {
    vec![
        remap_preset_card(
            state,
            "nav-to-xbox",
            "Nav to Xbox",
            "xbox360",
            serde_json::json!({
                "leftX": {"source": 0},
                "leftY": {"source": 1, "invert": true},
                "leftTrigger": {"source": 2},
                "rightTrigger": {"source": 5}
            }),
            serde_json::json!({
                "cross": "a",
                "circle": "b",
                "square": "x",
                "triangle": "y",
                "select": "back",
                "ps": "guide"
            }),
        ),
        remap_preset_card(
            state,
            "invert-y",
            "Invert Y",
            "xbox360",
            serde_json::json!({
                "leftX": {"source": 0},
                "leftY": {"source": 1, "invert": true},
                "leftTrigger": {"source": 2},
                "rightTrigger": {"source": 5}
            }),
            serde_json::json!({}),
        ),
    ]
}

fn remap_preset_card(
    state: &SleipnirRuntimeState,
    preset_id: &str,
    title: &str,
    presentation: &str,
    axis_map: serde_json::Value,
    button_map: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "id": format!("{}.remap.{}", state.provider_id, preset_id),
        "kind": "card",
        "props": {
            "title": title,
            "commandId": "sleipnir.apply-remap-preset",
            "action": {
                "type": "sleipnir.apply-remap-preset",
                "providerId": state.provider_id,
                "deviceFilter": state.selected_device_filter,
                "presentation": presentation,
                "axisMap": axis_map,
                "buttonMap": button_map
            }
        },
        "children": [
            text_element(format!("{}.remap.{}.detail", state.provider_id, preset_id), format!("presentation: {presentation}"))
        ]
    })
}

fn xbox_binding_map(state: &SleipnirRuntimeState) -> serde_json::Value {
    let targets = [
        (
            "leftX", "axis", "Left X", "axis 0", "16%", "34%", "265", "224",
        ),
        (
            "leftY", "axis", "Left Y", "axis 1", "16%", "49%", "265", "224",
        ),
        (
            "leftTrigger",
            "axis",
            "LT",
            "axis 2",
            "20%",
            "12%",
            "253",
            "68",
        ),
        (
            "rightTrigger",
            "axis",
            "RT",
            "axis 5",
            "80%",
            "12%",
            "527",
            "68",
        ),
        ("a", "button", "A", "unmapped", "88%", "72%", "568", "244"),
        ("b", "button", "B", "unmapped", "92%", "58%", "608", "204"),
        ("x", "button", "X", "unmapped", "80%", "58%", "528", "204"),
        ("y", "button", "Y", "unmapped", "84%", "44%", "568", "164"),
        ("lb", "button", "LB", "unmapped", "22%", "22%", "253", "108"),
        ("rb", "button", "RB", "unmapped", "78%", "22%", "527", "108"),
        (
            "back", "button", "Back", "unmapped", "40%", "43%", "363", "197",
        ),
        (
            "start", "button", "Start", "unmapped", "60%", "43%", "419", "197",
        ),
        (
            "guide", "button", "Guide", "unmapped", "50%", "36%", "391", "224",
        ),
        ("l3", "button", "L3", "unmapped", "30%", "76%", "265", "224"),
        ("r3", "button", "R3", "unmapped", "70%", "76%", "474", "256"),
        (
            "up", "button", "D-Up", "unmapped", "12%", "64%", "245", "224",
        ),
        (
            "down", "button", "D-Down", "unmapped", "12%", "78%", "245", "306",
        ),
        (
            "left", "button", "D-Left", "unmapped", "12%", "71%", "204", "267",
        ),
        (
            "right", "button", "D-Right", "unmapped", "26%", "71%", "286", "267",
        ),
    ];
    let children = targets
        .into_iter()
        .map(
            |(target, kind, label, fallback, x, y, anchor_x, anchor_y)| {
                binding_target(
                    state,
                    target,
                    kind,
                    label,
                    binding_summary(state, target, kind, fallback),
                    x,
                    y,
                    anchor_x,
                    anchor_y,
                )
            },
        )
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": format!("{}.binding-map", state.provider_id),
        "kind": "input.binding-map",
        "props": {
            "label": "Xbox controller remapping"
        },
        "children": children
    })
}

fn binding_target(
    state: &SleipnirRuntimeState,
    target: &str,
    binding_kind: &str,
    label: &str,
    current: String,
    x: &str,
    y: &str,
    anchor_x: &str,
    anchor_y: &str,
) -> serde_json::Value {
    let listening = state
        .pending_learn
        .as_ref()
        .is_some_and(|learn| learn.target == target && learn.binding_kind == binding_kind);
    serde_json::json!({
        "id": format!("{}.binding.{}", state.provider_id, stable_id(target)),
        "kind": "binding.target",
        "props": {
            "target": target,
            "bindingKind": binding_kind,
            "label": label,
            "current": if listening { "listening..." } else { current.as_str() },
            "x": x,
            "y": y,
            "anchorX": anchor_x,
            "anchorY": anchor_y,
            "documentId": format!("cultmesh://sleipnir/{}/binding/{target}", state.provider_id),
            "slotId": format!("sleipnir.binding.{target}"),
            "schemaId": "gamecult.eve.surface.v1",
            "presentationKind": "modal.binding",
            "deviceFilter": state.selected_device_filter,
            "presentation": "xbox360"
        },
        "embeddedDocuments": [{
            "slotId": format!("sleipnir.binding.{target}"),
            "documentId": format!("cultmesh://sleipnir/{}/binding/{target}", state.provider_id),
            "schemaId": "gamecult.eve.surface.v1",
            "presentationKind": "modal.binding"
        }]
    })
}

fn binding_summary(
    state: &SleipnirRuntimeState,
    target: &str,
    binding_kind: &str,
    fallback: &str,
) -> String {
    if binding_kind == "axis" {
        return axis_binding_label(state.axis_map.get(target));
    }
    let sources = state
        .button_map
        .iter()
        .filter_map(|(source, mapped)| (mapped == target).then_some(source.as_str()))
        .collect::<Vec<_>>();
    if sources.is_empty() {
        fallback.to_string()
    } else {
        sources.join(", ")
    }
}

fn axis_binding_label(binding: Option<&AxisBinding>) -> String {
    match binding {
        Some(binding) if binding.invert => format!("axis {} inverted", binding.source),
        Some(binding) => format!("axis {}", binding.source),
        None => "unmapped".to_string(),
    }
}

fn text_element(id: String, text: String) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "kind": "text",
        "props": { "text": text },
        "children": []
    })
}

fn read_desired_mapping(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
) -> SleipnirDesiredMapping {
    read_desired_mapping_from_sources(node, None, options)
}

fn read_desired_mapping_from_sources(
    local_node: &mut cultmesh_rs::CultMeshNode,
    discovery_node: Option<&mut cultmesh_rs::CultMeshNode>,
    options: &Options,
) -> SleipnirDesiredMapping {
    let local_record = read_desired_mapping_record(local_node, options);
    let discovery_record =
        discovery_node.and_then(|node| read_desired_mapping_record(node, options));
    if let Some(record) = newest_sleipnir_mapping_record(local_record, discovery_record) {
        return desired_mapping_from_record(record);
    }
    options
        .mapping_path
        .as_ref()
        .and_then(|path| read_legacy_mapping_file(path))
        .unwrap_or_else(default_desired_mapping)
}

fn read_desired_mapping_record(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
) -> Option<SleipnirInputMappingRecord> {
    let _ = node.pull_all_backing_stores();
    node.get::<SleipnirInputMappingRecord>(&sleipnir_provider_id(options))
        .ok()
        .flatten()
}

fn newest_sleipnir_mapping_record(
    local: Option<SleipnirInputMappingRecord>,
    discovery: Option<SleipnirInputMappingRecord>,
) -> Option<SleipnirInputMappingRecord> {
    match (local, discovery) {
        (Some(local), Some(discovery)) if discovery.updated_at > local.updated_at => {
            Some(discovery)
        }
        (Some(local), Some(_)) => Some(local),
        (Some(local), None) => Some(local),
        (None, Some(discovery)) => Some(discovery),
        (None, None) => None,
    }
}

fn default_desired_mapping() -> SleipnirDesiredMapping {
    SleipnirDesiredMapping {
        enabled: false,
        device_filter: None,
        stream_id: None,
        presentation: default_presentation(),
        axis_map: default_axis_map(),
        button_map: default_button_map(),
        pending_learn: None,
    }
}

fn desired_mapping_from_record(record: SleipnirInputMappingRecord) -> SleipnirDesiredMapping {
    SleipnirDesiredMapping {
        enabled: record.enabled,
        device_filter: Some(record.device_filter)
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string()),
        stream_id: Some(record.stream_id)
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string()),
        presentation: normalize_presentation(&record.presentation),
        axis_map: parse_axis_map(Some(&record.axis_map)),
        button_map: parse_button_map(Some(&record.button_map)),
        pending_learn: parse_learn_request(Some(&record.pending_learn)),
    }
}

fn read_legacy_mapping_file(path: &PathBuf) -> Option<SleipnirDesiredMapping> {
    let text = fs::read_to_string(path).ok()?;
    let value =
        serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok()?;
    Some(SleipnirDesiredMapping {
        enabled: false,
        device_filter: value
            .get("deviceFilter")
            .or_else(|| value.get("device_filter"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string),
        stream_id: value
            .get("streamId")
            .or_else(|| value.get("stream_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string),
        presentation: normalize_presentation(
            value
                .get("presentation")
                .or_else(|| value.get("presentationProfile"))
                .or_else(|| value.get("presentation_profile"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("xbox360"),
        ),
        axis_map: parse_axis_map(value.get("axisMap").or_else(|| value.get("axis_map"))),
        button_map: parse_button_map(value.get("buttonMap").or_else(|| value.get("button_map"))),
        pending_learn: parse_learn_request(
            value
                .get("pendingLearn")
                .or_else(|| value.get("pending_learn"))
                .or_else(|| value.get("learn")),
        ),
    })
}

fn parse_learn_request(value: Option<&serde_json::Value>) -> Option<LearnRequest> {
    let map = value?.as_object()?;
    let target = map
        .get("target")
        .or_else(|| map.get("buttonTarget"))
        .or_else(|| map.get("axisTarget"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())?
        .to_string();
    let binding_kind = map
        .get("bindingKind")
        .or_else(|| map.get("kind"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("button")
        .to_string();
    Some(LearnRequest {
        target,
        binding_kind,
        requested_at: map
            .get("requestedAt")
            .or_else(|| map.get("requested_at"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    })
}

fn default_presentation() -> String {
    "xbox360".to_string()
}

fn normalize_presentation(_presentation: &str) -> String {
    default_presentation()
}

fn default_axis_map() -> HashMap<String, AxisBinding> {
    HashMap::from([
        ("leftX".to_string(), AxisBinding::stick_source(0)),
        (
            "leftY".to_string(),
            AxisBinding {
                source: 1,
                invert: true,
                scale: 1.0,
                deadzone: DEFAULT_STICK_DEADZONE,
            },
        ),
        ("leftTrigger".to_string(), AxisBinding::source(2)),
        ("rightTrigger".to_string(), AxisBinding::source(5)),
    ])
}

fn default_button_map() -> HashMap<String, String> {
    HashMap::from([
        ("trigger".to_string(), "none".to_string()),
        ("l2".to_string(), "none".to_string()),
    ])
}

impl AxisBinding {
    fn source(source: usize) -> Self {
        Self {
            source,
            invert: false,
            scale: 1.0,
            deadzone: 0.0,
        }
    }

    fn stick_source(source: usize) -> Self {
        Self {
            source,
            invert: false,
            scale: 1.0,
            deadzone: DEFAULT_STICK_DEADZONE,
        }
    }
}

fn parse_axis_map(value: Option<&serde_json::Value>) -> HashMap<String, AxisBinding> {
    let mut axis_map = default_axis_map();
    let Some(map) = value.and_then(|value| value.as_object()) else {
        return axis_map;
    };
    for (target, binding) in map {
        if let Some(binding) = parse_axis_binding(binding) {
            axis_map.insert(target.clone(), binding);
        }
    }
    axis_map
}

fn parse_axis_binding(value: &serde_json::Value) -> Option<AxisBinding> {
    if let Some(source) = value.as_u64() {
        return Some(AxisBinding::source(source as usize));
    }
    let map = value.as_object()?;
    let source = map
        .get("source")
        .or_else(|| map.get("axis"))
        .and_then(|value| value.as_u64())? as usize;
    Some(AxisBinding {
        source,
        invert: map
            .get("invert")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        scale: map
            .get("scale")
            .and_then(|value| value.as_f64())
            .unwrap_or(1.0) as f32,
        deadzone: map
            .get("deadzone")
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0) as f32,
    })
}

fn parse_button_map(value: Option<&serde_json::Value>) -> HashMap<String, String> {
    value
        .and_then(|value| value.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(source, target)| {
                    target
                        .as_str()
                        .filter(|target| !target.trim().is_empty())
                        .map(|target| (source.clone(), target.to_string()))
                })
                .collect()
        })
        .unwrap_or_else(default_button_map)
}

fn apply_pending_learn(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    mapping: &SleipnirDesiredMapping,
    record: &MuninnHidControllerStateRecord,
) -> Result<Option<SleipnirDesiredMapping>> {
    let Some(learn) = mapping.pending_learn.as_ref() else {
        return Ok(None);
    };
    let mut updated = mapping.clone();
    let learned = if learn.binding_kind == "axis" {
        if let Some(source) = most_active_axis(&record.axes) {
            updated.axis_map.insert(
                learn.target.clone(),
                AxisBinding {
                    source,
                    invert: learn.target == "leftY" || learn.target == "rightY",
                    scale: 1.0,
                    deadzone: 0.0,
                },
            );
            true
        } else {
            false
        }
    } else if let Some(source) = record.buttons.first() {
        updated
            .button_map
            .insert(source.clone(), learn.target.clone());
        true
    } else {
        false
    };
    if !learned {
        return Ok(None);
    }
    updated.pending_learn = None;
    updated.enabled = true;
    write_desired_mapping(node, options, &updated, "sleipnir.auto-bind")?;
    Ok(Some(updated))
}

fn most_active_axis(axes: &[f32]) -> Option<usize> {
    axes.iter()
        .enumerate()
        .filter(|(_, value)| value.abs() >= 0.45)
        .max_by(|(_, left), (_, right)| {
            left.abs()
                .partial_cmp(&right.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
}

fn write_desired_mapping(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    mapping: &SleipnirDesiredMapping,
    source: &str,
) -> Result<()> {
    let provider_id = sleipnir_provider_id(options);
    let record = SleipnirInputMappingRecord {
        provider_id: provider_id.clone(),
        enabled: mapping.enabled,
        device_filter: mapping.device_filter.clone().unwrap_or_default(),
        stream_id: mapping.stream_id.clone().unwrap_or_default(),
        presentation: mapping.presentation.clone(),
        axis_map: axis_map_json(&mapping.axis_map),
        button_map: serde_json::Value::Object(
            mapping
                .button_map
                .iter()
                .map(|(source, target)| (source.clone(), serde_json::Value::String(target.clone())))
                .collect(),
        ),
        pending_learn: mapping
            .pending_learn
            .as_ref()
            .map(|learn| {
                serde_json::json!({
                    "target": learn.target,
                    "bindingKind": learn.binding_kind,
                    "requestedAt": learn.requested_at
                })
            })
            .unwrap_or(serde_json::Value::Null),
        updated_at: timestamp(),
        source: source.to_string(),
    };
    node.put(&provider_id, &record)?;
    Ok(())
}

fn axis_map_json(axis_map: &HashMap<String, AxisBinding>) -> serde_json::Value {
    serde_json::Value::Object(
        axis_map
            .iter()
            .map(|(target, binding)| {
                (
                    target.clone(),
                    serde_json::json!({
                        "source": binding.source,
                        "invert": binding.invert,
                        "scale": binding.scale,
                        "deadzone": binding.deadzone
                    }),
                )
            })
            .collect(),
    )
}

fn effective_muninn_hid_endpoint(
    node: &cultmesh_rs::CultMeshNode,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
    trace: bool,
) -> Option<String> {
    discover_muninn_hid_endpoint(node, device_filter, stream_id, trace)
}

fn effective_device_filter(
    options: &Options,
    desired_mapping: &SleipnirDesiredMapping,
) -> Option<String> {
    desired_mapping
        .device_filter
        .clone()
        .or_else(|| options.device_filter.clone())
}

fn discover_available_hid_devices(node: &cultmesh_rs::CultMeshNode) -> Vec<AvailableHidDevice> {
    let mut devices = Vec::new();
    for record in node
        .cache()
        .get_all::<MuninnHidControllerStateRecord>()
        .ok()
        .into_iter()
        .flatten()
    {
        upsert_available_hid_device(
            &mut devices,
            available_hid_device_from_record(
                node,
                &record,
                discover_muninn_hid_endpoint(
                    node,
                    Some(record.device_id.as_str()),
                    Some(record.stream_id.as_str()),
                    false,
                ),
            ),
        );
    }
    for provider in node
        .cache()
        .get_all::<EveProviderAdvertisementRecord>()
        .ok()
        .into_iter()
        .flatten()
    {
        for device in available_hid_devices_from_value(&provider.value) {
            upsert_available_hid_device(&mut devices, device);
        }
    }
    for envelope in node.cache().snapshot() {
        if envelope.schema_id.as_deref() == Some(MUNINN_HID_CONTROLLER_STATE_SCHEMA)
            || envelope.r#type == "muninn.hid_controller_state"
        {
            if let Ok(value) = rmp_serde::from_slice::<serde_json::Value>(&envelope.payload) {
                if let Some(device) = available_hid_device_from_hid_value(node, &value) {
                    upsert_available_hid_device(&mut devices, device);
                }
            }
            continue;
        }
        if envelope.schema_id.as_deref() == Some(EVE_PROVIDER_ADVERTISEMENT_SCHEMA)
            || envelope.r#type == "gamecult.eve.provider_advertisement"
        {
            if let Ok(value) = rmp_serde::from_slice::<serde_json::Value>(&envelope.payload) {
                for device in available_hid_devices_from_value(&value) {
                    upsert_available_hid_device(&mut devices, device);
                }
            }
            continue;
        }
    }
    devices.retain(|device| endpoint_looks_like_socket(&device.endpoint));
    devices.sort_by(|left, right| {
        (&left.host_id, &left.device_id, &left.endpoint).cmp(&(
            &right.host_id,
            &right.device_id,
            &right.endpoint,
        ))
    });
    devices
        .dedup_by(|left, right| left.host_id == right.host_id && left.device_id == right.device_id);
    devices
}

fn available_hid_device_from_hid_value(
    node: &cultmesh_rs::CultMeshNode,
    value: &serde_json::Value,
) -> Option<AvailableHidDevice> {
    let payload = value
        .as_array()
        .and_then(|items| {
            (items.len() == 1)
                .then(|| items.first())
                .flatten()
                .filter(|item| item.is_object() || item.is_array())
        })
        .unwrap_or(value);
    let (stream_id, host_id, device_id, device_kind, source_path) =
        if let Some(items) = payload.as_array() {
            (
                items.first()?.as_str()?.to_string(),
                items.get(1)?.as_str()?.to_string(),
                items.get(2)?.as_str()?.to_string(),
                items.get(3)?.as_str()?.to_string(),
                items
                    .get(10)
                    .and_then(|item| item.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            )
        } else {
            (
                string_field(payload, &["stream_id", "streamId"])?,
                string_field(payload, &["host_id", "hostId"])?,
                string_field(payload, &["device_id", "deviceId"])?,
                string_field(payload, &["device_kind", "deviceKind"])?,
                string_field(payload, &["source_path", "sourcePath"])
                    .unwrap_or_else(|| "unknown".to_string()),
            )
        };
    let endpoint = discover_muninn_hid_endpoint(
        node,
        Some(device_id.as_str()),
        Some(stream_id.as_str()),
        false,
    )
    .unwrap_or_else(|| "discovering".to_string());
    Some(AvailableHidDevice {
        device_id,
        device_kind,
        source_path,
        stream_id,
        host_id,
        endpoint,
    })
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_str()))
        .map(ToString::to_string)
}

fn available_hid_device_from_record(
    node: &cultmesh_rs::CultMeshNode,
    record: &MuninnHidControllerStateRecord,
    endpoint: Option<String>,
) -> AvailableHidDevice {
    AvailableHidDevice {
        device_id: record.device_id.clone(),
        device_kind: record.device_kind.clone(),
        source_path: record.source_path.clone(),
        stream_id: record.stream_id.clone(),
        host_id: record.host_id.clone(),
        endpoint: endpoint.unwrap_or_else(|| {
            discover_muninn_hid_endpoint(
                node,
                Some(record.device_id.as_str()),
                Some(record.stream_id.as_str()),
                false,
            )
            .unwrap_or_else(|| "discovering".to_string())
        }),
    }
}

fn upsert_available_hid_device(devices: &mut Vec<AvailableHidDevice>, device: AvailableHidDevice) {
    if let Some(existing) = devices.iter_mut().find(|existing| {
        existing.host_id == device.host_id && existing.device_id == device.device_id
    }) {
        if should_replace_available_hid_device(existing, &device) {
            *existing = device;
        }
    } else {
        devices.push(device);
    }
    devices.sort_by(|left, right| {
        (&left.host_id, &left.device_id, &left.endpoint).cmp(&(
            &right.host_id,
            &right.device_id,
            &right.endpoint,
        ))
    });
}

fn should_replace_available_hid_device(
    existing: &AvailableHidDevice,
    candidate: &AvailableHidDevice,
) -> bool {
    let existing_score = available_hid_device_discovery_score(existing);
    let candidate_score = available_hid_device_discovery_score(candidate);
    candidate_score >= existing_score
}

fn available_hid_device_discovery_score(device: &AvailableHidDevice) -> u8 {
    let mut score = 0;
    if device.endpoint != "discovering" {
        score += 4;
    }
    if device.stream_id.contains(&device.device_id) {
        score += 2;
    }
    if !device.source_path.trim().is_empty() && device.source_path != "unknown" {
        score += 1;
    }
    score
}

fn available_hid_devices_from_value(value: &serde_json::Value) -> Vec<AvailableHidDevice> {
    let mut devices = Vec::new();
    let Some(streams) = value
        .get("inputStreams")
        .or_else(|| value.get("input_streams"))
        .and_then(|streams| streams.as_array())
    else {
        return devices;
    };
    for stream in streams {
        let endpoint = stream
            .get("address")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let stream_id = stream
            .get("streamId")
            .or_else(|| stream.get("stream_id"))
            .and_then(|value| value.as_str())
            .unwrap_or("muninn:hid")
            .to_string();
        if !is_muninn_fast_hid_stream(stream, &stream_id, &endpoint) {
            continue;
        }
        let host_id = value
            .get("verseId")
            .or_else(|| value.get("verse_id"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim_end_matches(".local")
            .to_string();
        if let Some(stream_devices) = stream.get("devices").and_then(|value| value.as_array()) {
            for device in stream_devices {
                let device_id = device
                    .get("deviceId")
                    .or_else(|| device.get("device_id"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                if device_id.is_empty() {
                    continue;
                }
                devices.push(AvailableHidDevice {
                    device_id,
                    device_kind: device
                        .get("deviceKind")
                        .or_else(|| device.get("device_kind"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("hid")
                        .to_string(),
                    source_path: device
                        .get("sourcePath")
                        .or_else(|| device.get("source_path"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                    stream_id: stream_id.clone(),
                    host_id: host_id.clone(),
                    endpoint: endpoint.clone(),
                });
            }
        }
    }
    devices
}

fn is_muninn_fast_hid_stream(stream: &serde_json::Value, stream_id: &str, endpoint: &str) -> bool {
    if stream.get("schema").and_then(|value| value.as_str())
        != Some("muninn.hid_controller_state.v1")
    {
        return false;
    }
    if !stream_id.contains(":hid-controller-state") || stream_id.contains("move-evidence") {
        return false;
    }
    if !endpoint_looks_like_socket(endpoint) {
        return false;
    }
    stream
        .get("transport")
        .and_then(|value| value.as_str())
        .is_some_and(|transport| transport.contains("rudp"))
}

fn endpoint_looks_like_socket(endpoint: &str) -> bool {
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return false;
    };
    !host.trim().is_empty()
        && port
            .parse::<u16>()
            .is_ok_and(|parsed_port| parsed_port != 0)
}

fn stable_id(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_ascii_lowercase();
    if normalized.is_empty() {
        "device".to_string()
    } else {
        normalized
    }
}

fn sleipnir_provider_id(options: &Options) -> String {
    format!("sleipnir.input-mirror.{}", options.host_id)
}

fn sleipnir_daemon_id(options: &Options) -> String {
    format!("{}-sleipnir", options.host_id)
}

fn sleipnir_provider_title(options: &Options) -> String {
    format!("Sleipnir {} Input Mirror", title_case(&options.host_id))
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("unix-{}", duration.as_secs()),
        Err(_) => "unix-0".to_string(),
    }
}

fn unix_millis_i64() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn unix_nanos_i64() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn create_rudp_stream(
    bind: Option<SocketAddr>,
    connect_endpoint: Option<String>,
    trace: bool,
) -> Result<Option<ActiveRudpStream>> {
    if let Some(endpoint) = connect_endpoint {
        let target = resolve_socket_addr(&endpoint)
            .with_context(|| format!("resolving Muninn HID RUDP endpoint {endpoint}"))?;
        let bind_address = bind.unwrap_or_else(|| {
            if target.is_ipv4() {
                SocketAddr::from(([0, 0, 0, 0], 0))
            } else {
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0))
            }
        });
        let socket = UdpSocket::bind(bind_address)
            .with_context(|| format!("binding Sleipnir RUDP HID client at {bind_address}"))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(1)))
            .context("setting Sleipnir RUDP HID client timeout")?;
        if trace {
            eprintln!(
                "Sleipnir connecting to Muninn HID RUDP at {target} connection_id={SLEIPNIR_HID_RUDP_CONNECTION_ID:08x}"
            );
        }
        let mut transport =
            CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
                runtime_id: "sleipnir-hid-rudp".to_string(),
                socket,
                mode: CultNetRudpSocketMode::Client,
                remote_addr: Some(target),
                connection_id: SLEIPNIR_HID_RUDP_CONNECTION_ID,
                initial_sequence: 1,
                resend_delay_ms: 5,
                transport_id: Some("sleipnir-hid-rudp".to_string()),
                max_payload_bytes: None,
                max_fragment_bytes: Some(HID_RUDP_MAX_FRAGMENT_BYTES),
                max_pending_reliable_packets: Some(256),
                reconnect_policy: None,
            })?;
        transport.connect(Vec::new())?;
        return Ok(Some(ActiveRudpStream {
            transport,
            target: Some(target),
            last_connect_attempt_at: Some(Instant::now()),
            connected_logged: false,
            last_frame_at: Some(Instant::now()),
            last_stale_log_at: None,
            last_subscription: None,
            last_subscription_at: None,
            received_edges: Vec::new(),
        }));
    }
    let Some(bind) = bind else {
        return Ok(None);
    };
    let socket = UdpSocket::bind(bind)
        .with_context(|| format!("binding Sleipnir RUDP HID stream at {bind}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(1)))
        .with_context(|| format!("setting Sleipnir RUDP HID stream {bind} timeout"))?;
    if trace {
        eprintln!(
            "Sleipnir listening for fast HID RUDP on {bind} connection_id={SLEIPNIR_HID_RUDP_CONNECTION_ID:08x}"
        );
    }
    Ok(Some(ActiveRudpStream {
        transport: CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "sleipnir-hid-rudp".to_string(),
            socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id: SLEIPNIR_HID_RUDP_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 5,
            transport_id: Some("sleipnir-hid-rudp".to_string()),
            max_payload_bytes: None,
            max_fragment_bytes: Some(HID_RUDP_MAX_FRAGMENT_BYTES),
            max_pending_reliable_packets: Some(256),
            reconnect_policy: None,
        })?,
        target: None,
        last_connect_attempt_at: None,
        connected_logged: false,
        last_frame_at: None,
        last_stale_log_at: None,
        last_subscription: None,
        last_subscription_at: None,
        received_edges: Vec::new(),
    }))
}

fn send_hid_subscription_if_due(
    stream: &mut ActiveRudpStream,
    device_filter: Option<String>,
    stream_id: Option<String>,
    trace: bool,
) -> Result<()> {
    if !stream.transport.connected() {
        return Ok(());
    }
    let subscription = HidControllerRudpSubscription {
        device_filter: device_filter.filter(|filter| !filter.trim().is_empty()),
        stream_id: stream_id.filter(|stream_id| !stream_id.trim().is_empty()),
    };
    let changed = stream.last_subscription.as_ref() != Some(&subscription);
    let due = stream
        .last_subscription_at
        .is_none_or(|sent_at| sent_at.elapsed() >= Duration::from_secs(1));
    if !changed && !due {
        return Ok(());
    }
    let payload = serde_json::to_vec(&serde_json::json!({
        "deviceFilter": subscription.device_filter,
        "streamId": subscription.stream_id,
    }))
    .context("encoding Sleipnir HID RUDP subscription")?;
    stream
        .transport
        .send("hid.subscribe", payload)
        .context("sending Sleipnir HID RUDP subscription")?;
    if trace || changed {
        eprintln!(
            "Sleipnir subscribed to Muninn HID device_filter={:?} stream_id={:?}",
            subscription.device_filter, subscription.stream_id
        );
    }
    stream.last_subscription = Some(subscription);
    stream.last_subscription_at = Some(Instant::now());
    Ok(())
}

fn resolve_socket_addr(endpoint: &str) -> Result<SocketAddr> {
    endpoint
        .to_socket_addrs()
        .with_context(|| format!("resolving {endpoint}"))?
        .next()
        .ok_or_else(|| anyhow!("no socket addresses resolved for {endpoint}"))
}

fn ensure_rudp_connected(stream: &mut ActiveRudpStream, trace: bool) -> Result<bool> {
    let Some(target) = stream.target else {
        return Ok(false);
    };
    if stream.transport.connected() {
        if !stream.connected_logged && trace {
            eprintln!("Sleipnir connected to Muninn HID RUDP at {target}");
        }
        stream.connected_logged = true;
        if stream
            .last_frame_at
            .is_some_and(|last_frame| last_frame.elapsed() >= INPUT_STREAM_RECONNECT_AFTER)
        {
            let should_log = stream
                .last_stale_log_at
                .is_none_or(|logged| logged.elapsed() >= INPUT_STREAM_RECONNECT_AFTER);
            if should_log && trace {
                eprintln!(
                    "Sleipnir Muninn HID RUDP is connected to {target} but has received no frames for {:?}; recreating connection",
                    stream.last_frame_at.map(|last_frame| last_frame.elapsed())
                );
            }
            stream.last_stale_log_at = Some(Instant::now());
            stream.connected_logged = false;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
    let should_attempt = stream
        .last_connect_attempt_at
        .is_none_or(|attempt| attempt.elapsed() >= Duration::from_millis(250));
    if should_attempt {
        stream.last_connect_attempt_at = Some(Instant::now());
        stream.transport.connect(Vec::new())?;
    }
    Ok(false)
}

fn discover_muninn_hid_endpoint(
    node: &cultmesh_rs::CultMeshNode,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
    trace: bool,
) -> Option<String> {
    if let Some(endpoint) = discover_available_hid_endpoint(node, device_filter, stream_id) {
        return Some(endpoint);
    }
    let host_hint = stream_host_hint(stream_id);
    let endpoints = discover_provider_endpoints(
        node,
        OdinEndpointQuery {
            schema: Some(MUNINN_HID_CONTROLLER_STATE_SCHEMA),
            transport_contains: Some("rudp"),
            host_hint: host_hint.as_deref(),
            device_filter,
        },
    );
    if trace && endpoints.is_empty() {
        eprintln!(
            "Sleipnir found no Muninn HID endpoints for host_hint={:?} device_filter={:?}",
            host_hint, device_filter
        );
    }
    endpoints
        .into_iter()
        .next()
        .map(|endpoint| endpoint.address)
}

fn discover_available_hid_endpoint(
    node: &cultmesh_rs::CultMeshNode,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
) -> Option<String> {
    for provider in node
        .cache()
        .get_all::<EveProviderAdvertisementRecord>()
        .ok()
        .into_iter()
        .flatten()
    {
        if let Some(endpoint) =
            matching_provider_advertised_hid_endpoint(&provider.value, device_filter, stream_id)
        {
            return Some(endpoint);
        }
    }
    for envelope in node.cache().snapshot() {
        if envelope.schema_id.as_deref() != Some(EVE_PROVIDER_ADVERTISEMENT_SCHEMA)
            && envelope.r#type != "gamecult.eve.provider_advertisement"
        {
            continue;
        }
        let Ok(value) = rmp_serde::from_slice::<serde_json::Value>(&envelope.payload) else {
            continue;
        };
        if let Some(endpoint) =
            matching_provider_advertised_hid_endpoint(&value, device_filter, stream_id)
        {
            return Some(endpoint);
        }
    }
    None
}

fn matching_provider_advertised_hid_endpoint(
    value: &serde_json::Value,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
) -> Option<String> {
    let host_hint = stream_host_hint(stream_id);
    let filter = device_filter
        .map(str::trim)
        .filter(|value| !value.is_empty());
    available_hid_devices_from_value(value)
        .into_iter()
        .filter(|device| {
            host_hint
                .as_deref()
                .is_none_or(|host| device.host_id.eq_ignore_ascii_case(host))
        })
        .find(|device| {
            stream_id.is_none_or(|stream_id| device.stream_id == stream_id)
                || filter.is_some_and(|filter| {
                    device.device_id == filter
                        || device.device_kind == filter
                        || device.stream_id == filter
                        || device.source_path.contains(filter)
                })
        })
        .map(|device| device.endpoint)
}

fn stream_host_hint(stream_id: Option<&str>) -> Option<String> {
    let stream_id = stream_id?;
    let parts = stream_id
        .split(':')
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    let host = match parts.as_slice() {
        ["muninn", host, ..] => *host,
        [host, ..] => *host,
        _ => return None,
    };
    let normalized = host.trim().trim_end_matches(".local").to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn receive_rudp_records(stream: &mut ActiveRudpStream, trace: bool) -> Result<Vec<TimedHidRecord>> {
    let mut records = Vec::new();
    let mut empty_polls = 0usize;
    for _ in 0..64 {
        match stream.transport.receive_once() {
            Ok(Some(frame)) if frame.channel_id == "latest" || frame.channel_id == "hid" => {
                empty_polls = 0;
                let frame_received_at = Instant::now();
                let frame_received_unix_ns = unix_nanos_i64();
                if trace {
                    eprintln!(
                        "Sleipnir received fast HID frame channel={} bytes={}",
                        frame.channel_id,
                        frame.payload.len()
                    );
                }
                let decoded = decode_latest_state_frame(&frame.payload);
                let record = match decoded.as_ref() {
                    Some(frame) => frame.record.clone(),
                    None => serde_json::from_slice(&frame.payload).context("decoding Sleipnir RUDP HID frame")?,
                };
                records.push(TimedHidRecord {
                    state_sequence: decoded.as_ref().map_or(record.sequence, |frame| frame.state_sequence),
                    epoch: decoded.as_ref().map_or(0, |frame| frame.epoch),
                    record,
                    timing: HidRecordTiming {
                        frame_received_at,
                        frame_received_unix_ns,
                        buffer_ready_at: frame_received_at,
                    },
                });
            }
            Ok(Some(frame)) if frame.channel_id == "hid.edge" => {
                empty_polls = 0;
                let edge = decode_button_edge(&frame.payload)
                    .ok_or_else(|| anyhow!("decoding Sleipnir HID button edge"))?;
                stream.received_edges.push(edge);
            }
            Ok(Some(frame)) => {
                empty_polls = 0;
                if trace {
                    eprintln!(
                        "Sleipnir ignored RUDP channel={} bytes={}",
                        frame.channel_id,
                        frame.payload.len()
                    );
                }
            }
            Ok(None) => {
                empty_polls += 1;
                if empty_polls >= 4 {
                    break;
                }
            }
            Err(error) => {
                if trace {
                    eprintln!("Sleipnir RUDP receive/control warning: {error:#}");
                }
                break;
            }
        }
    }
    if let Err(error) = stream.transport.poll_resends() {
        if trace {
            eprintln!("Sleipnir RUDP resend warning: {error:#}");
        }
    }
    let buffer_ready_at = Instant::now();
    let mut records = coalesce_latest_timed_hid_records_by_device(records);
    for record in &mut records {
        record.timing.buffer_ready_at = buffer_ready_at;
    }
    Ok(records)
}

fn coalesce_latest_timed_hid_records_by_device(
    records: Vec<TimedHidRecord>,
) -> Vec<TimedHidRecord> {
    let mut latest_by_device = HashMap::<String, (usize, TimedHidRecord)>::new();
    for (index, record) in records.into_iter().enumerate() {
        latest_by_device
            .entry(record.record.device_id.clone())
            .and_modify(|(latest_index, latest_record)| {
                if record.record.source_timestamp_ns > latest_record.record.source_timestamp_ns
                    || (record.record.source_timestamp_ns
                        == latest_record.record.source_timestamp_ns
                        && record.record.sequence > latest_record.record.sequence)
                {
                    *latest_index = index;
                    *latest_record = record.clone();
                }
            })
            .or_insert((index, record));
    }
    let mut latest = latest_by_device.into_values().collect::<Vec<_>>();
    latest.sort_by_key(|(index, _)| *index);
    latest.into_iter().map(|(_, record)| record).collect()
}

fn coalesce_latest_hid_records_by_device(
    records: Vec<MuninnHidControllerStateRecord>,
) -> Vec<MuninnHidControllerStateRecord> {
    let mut latest_by_device = HashMap::<String, (usize, MuninnHidControllerStateRecord)>::new();
    for (index, record) in records.into_iter().enumerate() {
        latest_by_device
            .entry(record.device_id.clone())
            .and_modify(|(latest_index, latest_record)| {
                if record.source_timestamp_ns > latest_record.source_timestamp_ns
                    || (record.source_timestamp_ns == latest_record.source_timestamp_ns
                        && record.sequence > latest_record.sequence)
                {
                    *latest_index = index;
                    *latest_record = record.clone();
                }
            })
            .or_insert((index, record));
    }
    let mut latest = latest_by_device.into_values().collect::<Vec<_>>();
    latest.sort_by_key(|(index, _)| *index);
    latest.into_iter().map(|(_, record)| record).collect()
}

#[cfg(windows)]
fn create_backend() -> Result<Box<dyn VirtualPadBackend>> {
    Ok(Box::new(VigemXboxBackend::connect()?))
}

#[cfg(not(windows))]
fn create_backend() -> Result<Box<dyn VirtualPadBackend>> {
    Err(anyhow!(
        "Sleipnir's ViGEm Xbox virtual controller backend is currently implemented on Windows; use --dry-run on this platform"
    ))
}

fn record_matches_filter(
    record: &MuninnHidControllerStateRecord,
    device_filter: Option<&str>,
) -> bool {
    device_filter.is_none_or(|filter| {
        record.device_id == filter
            || record.stream_id == filter
            || record.source_path.contains(filter)
            || record.device_kind == filter
    })
}

fn input_latency_snapshot(
    record: &MuninnHidControllerStateRecord,
    timing: &HidRecordTiming,
    axis_classified_at: Instant,
    hid_emitted_at: Instant,
    mapping: &SleipnirDesiredMapping,
    emitted: bool,
) -> SleipnirInputLatencySnapshot {
    let source_to_arrival_ms = (record.source_timestamp_ns > 0).then(|| {
        timing
            .frame_received_unix_ns
            .saturating_sub(record.source_timestamp_ns)
            / 1_000_000
    });
    let source_to_hid_ms = (record.source_timestamp_ns > 0)
        .then(|| unix_nanos_i64().saturating_sub(record.source_timestamp_ns) / 1_000_000);
    SleipnirInputLatencySnapshot {
        device_id: record.device_id.clone(),
        sequence: record.sequence,
        source_to_arrival_ms,
        arrival_to_buffer_ms: timing
            .buffer_ready_at
            .saturating_duration_since(timing.frame_received_at)
            .as_millis(),
        buffer_to_axis_ms: axis_classified_at
            .saturating_duration_since(timing.buffer_ready_at)
            .as_millis(),
        axis_to_hid_ms: hid_emitted_at
            .saturating_duration_since(axis_classified_at)
            .as_millis(),
        source_to_hid_ms,
        total_observed_ms: hid_emitted_at
            .saturating_duration_since(timing.frame_received_at)
            .as_millis(),
        axis_summary: axis_classification_summary(record, mapping),
        emitted,
    }
}

fn axis_classification_summary(
    record: &MuninnHidControllerStateRecord,
    mapping: &SleipnirDesiredMapping,
) -> Vec<String> {
    let mut axes = mapping
        .axis_map
        .iter()
        .map(|(target, binding)| {
            let value = record.axes.get(binding.source).copied();
            format!(
                "{}<-axis{}{}{}",
                target,
                binding.source,
                if binding.invert { " inverted" } else { "" },
                value
                    .map(|value| format!(" value={:.3}", value))
                    .unwrap_or_else(|| " missing".to_string())
            )
        })
        .collect::<Vec<_>>();
    axes.sort();
    axes
}

fn map_record_to_virtual_pad(
    record: &MuninnHidControllerStateRecord,
    mapping: &SleipnirDesiredMapping,
) -> VirtualPadState {
    let left_x = signed_axis_to_i16(mapped_axis(record, mapping, "leftX", 0.0));
    let left_y = signed_axis_to_i16(mapped_axis(record, mapping, "leftY", 0.0));
    let mut buttons = mapped_buttons(record, mapping);
    let left_trigger = mapped_trigger(record, mapping, "leftTrigger", 2);
    let right_trigger = mapped_trigger(record, mapping, "rightTrigger", 5);
    if buttons.iter().any(|button| button == "l2") && left_trigger == 0 {
        buttons.push("left-trigger-button".to_string());
    }
    VirtualPadState {
        buttons,
        left_x,
        left_y,
        left_trigger,
        right_trigger,
    }
}

fn mapped_axis(
    record: &MuninnHidControllerStateRecord,
    mapping: &SleipnirDesiredMapping,
    target: &str,
    default: f32,
) -> f32 {
    let Some(binding) = mapping.axis_map.get(target) else {
        return default;
    };
    let mut value = record.axes.get(binding.source).copied().unwrap_or(default);
    if value.abs() < binding.deadzone {
        value = 0.0;
    }
    if binding.invert {
        value = -value;
    }
    (value * binding.scale).clamp(-1.0, 1.0)
}

fn mapped_trigger(
    record: &MuninnHidControllerStateRecord,
    mapping: &SleipnirDesiredMapping,
    target: &str,
    fallback_source: usize,
) -> u8 {
    if mapping.axis_map.contains_key(target) {
        if mapping
            .axis_map
            .get(target)
            .is_some_and(|binding| record.axes.get(binding.source).is_none())
        {
            return 0;
        }
        trigger_from_axis(
            Some(mapped_axis(record, mapping, target, 0.0)),
            record.device_kind.as_str(),
        )
    } else {
        trigger_from_axis(
            record.axes.get(fallback_source).copied(),
            record.device_kind.as_str(),
        )
    }
}

fn mapped_buttons(
    record: &MuninnHidControllerStateRecord,
    mapping: &SleipnirDesiredMapping,
) -> Vec<String> {
    let mut buttons = Vec::new();
    for button in &record.buttons {
        let mapped = mapping
            .button_map
            .get(button)
            .map(String::as_str)
            .unwrap_or(button.as_str());
        if mapped.eq_ignore_ascii_case("none") || mapped.trim().is_empty() {
            continue;
        }
        if !buttons.iter().any(|existing| existing == mapped) {
            buttons.push(mapped.to_string());
        }
    }
    buttons
}

fn signed_axis_to_i16(value: f32) -> i16 {
    let clamped = value.clamp(-1.0, 1.0);
    if clamped < 0.0 {
        (clamped * 32768.0).round() as i16
    } else {
        (clamped * 32767.0).round() as i16
    }
}

fn trigger_from_axis(value: Option<f32>, device_kind: &str) -> u8 {
    let Some(value) = value else {
        return 0;
    };
    if device_kind == "xinput" || device_kind == "xinput-controller" {
        return (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    (((value.clamp(-1.0, 1.0) + 1.0) * 0.5) * 255.0).round() as u8
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Options {
            store_path: PathBuf::from("C:/Meta/Odin/state/starfire.muninn.telemetry.cc"),
            discovery_store_path: None,
            mapping_path: None,
            device_filter: None,
            host_id: "starfire".to_string(),
            interval_ms: 8,
            odin_cultmesh_uri: Some("cultmesh://odin/rendezvous/provider-catalog".to_string()),
            rudp_bind: None,
            command_route: SLEIPNIR_COMMAND_ROUTE.to_string(),
            idunn_rudp_health: None,
            once: false,
            dry_run: false,
            trace: env::var("SLEIPNIR_TRACE")
                .ok()
                .is_some_and(|value| value != "0" && !value.eq_ignore_ascii_case("false")),
        };
        let mut idunn_rudp_health_endpoint = None;
        let mut idunn_daemon_id = None;
        let mut idunn_health_contract = None;
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--store" => options.store_path = PathBuf::from(next_value(&mut args, "--store")?),
                "--discovery-store" => {
                    options.discovery_store_path =
                        Some(PathBuf::from(next_value(&mut args, "--discovery-store")?))
                }
                "--mapping" | "--mapping-path" => {
                    options.mapping_path = Some(PathBuf::from(next_value(&mut args, &arg)?));
                }
                "--device" => options.device_filter = Some(next_value(&mut args, "--device")?),
                "--host" => options.host_id = next_value(&mut args, "--host")?,
                "--interval-ms" => {
                    options.interval_ms = next_value(&mut args, "--interval-ms")?
                        .parse()
                        .context("--interval-ms must be an integer")?
                }
                "--rudp-bind" | "--udp-bind" => {
                    options.rudp_bind = Some(
                        next_value(&mut args, &arg)?
                            .parse()
                            .context("--rudp-bind must be a socket address")?,
                    )
                }
                "--command-rudp-bind" => {
                    let _ = next_value(&mut args, "--command-rudp-bind")?;
                    return Err(anyhow!(
                        "--command-rudp-bind has been removed; Sleipnir reads input mapping commands from Odin/CultMesh discovery"
                    ));
                }
                "--command-route" => {
                    options.command_route = next_value(&mut args, "--command-route")?
                }
                "--command-rudp-advertise" => {
                    return Err(anyhow!(
                        "--command-rudp-advertise has been removed; use --command-route with a cultmesh:// Odin command route"
                    ));
                }
                "--odin-cultmesh-rudp" => {
                    let _ = next_value(&mut args, "--odin-cultmesh-rudp")?;
                    return Err(anyhow!(
                        "--odin-cultmesh-rudp has been removed; use --odin-cultmesh-uri cultmesh://odin/rendezvous/provider-catalog and let CultMesh resolve Odin's transport"
                    ));
                }
                "--odin-cultmesh-uri" => {
                    options.odin_cultmesh_uri = Some(next_value(&mut args, "--odin-cultmesh-uri")?)
                }
                "--idunn-rudp-health" => {
                    idunn_rudp_health_endpoint = Some(
                        next_value(&mut args, "--idunn-rudp-health")?
                            .parse()
                            .context("--idunn-rudp-health must be a socket address")?,
                    )
                }
                "--idunn-daemon" => {
                    idunn_daemon_id = Some(next_value(&mut args, "--idunn-daemon")?)
                }
                "--idunn-health-contract" => {
                    idunn_health_contract = Some(next_value(&mut args, "--idunn-health-contract")?)
                }
                "--once" => options.once = true,
                "--dry-run" => options.dry_run = true,
                "--trace" => options.trace = true,
                "--help" | "-h" => {
                    println!("{}", usage());
                    std::process::exit(0);
                }
                other => return Err(anyhow!("unrecognized argument {other}\n{}", usage())),
            }
        }
        options.idunn_rudp_health = match (
            idunn_rudp_health_endpoint,
            idunn_daemon_id,
            idunn_health_contract,
        ) {
            (None, None, None) => None,
            (Some(endpoint), Some(daemon_id), Some(health_contract)) => {
                Some(IdunnRudpHealthOptions {
                    endpoint,
                    daemon_id,
                    health_contract,
                })
            }
            _ => {
                return Err(anyhow!(
                    "--idunn-rudp-health, --idunn-daemon, and --idunn-health-contract must be provided together"
                ));
            }
        };
        Ok(options)
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn usage() -> &'static str {
    "usage: sleipnir [--store PATH] [--discovery-store PATH] [--mapping-path LEGACY_PATH] [--host HOST] [--device DEVICE_OR_KIND] [--odin-cultmesh-uri cultmesh://...] [--rudp-bind ADDR] [--command-route cultmesh://...] [--idunn-rudp-health ADDR --idunn-daemon ID --idunn-health-contract CONTRACT] [--interval-ms N] [--trace] [--dry-run] [--once]"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn record(axes: Vec<f32>, buttons: Vec<&str>, sequence: u64) -> MuninnHidControllerStateRecord {
        MuninnHidControllerStateRecord {
            stream_id: "nightwing:nav:hid-controller-state".to_string(),
            host_id: "nightwing".to_string(),
            device_id: "nav".to_string(),
            device_kind: "ps3-navigation".to_string(),
            sequence,
            source_timestamp_ns: 0,
            axes,
            buttons: buttons.into_iter().map(String::from).collect(),
            battery01: f32::NAN,
            observed_at: "unix:0".to_string(),
            source_path: "/dev/input/js0".to_string(),
        }
    }

    fn default_mapping() -> SleipnirDesiredMapping {
        SleipnirDesiredMapping {
            enabled: false,
            device_filter: None,
            stream_id: None,
            presentation: default_presentation(),
            axis_map: default_axis_map(),
            button_map: default_button_map(),
            pending_learn: None,
        }
    }

    fn test_node(name: &str) -> (cultmesh_rs::CultMeshNode, PathBuf) {
        let store_path =
            std::env::temp_dir().join(format!("sleipnir-{name}-{}.cc", unix_millis_i64()));
        let node = CultMesh::create_node(
            &store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: format!("sleipnir-test-{name}"),
                pull_on_start: true,
            },
        )
        .unwrap();
        (node, store_path)
    }

    #[test]
    fn coalesces_latest_hid_records_per_device() {
        let mut old_nav = record(vec![1.0, 0.0, -1.0], Vec::new(), 1);
        old_nav.device_id = "nav".to_string();
        old_nav.source_timestamp_ns = 1_000;
        let mut xbox = record(vec![0.25, 0.0, -1.0], Vec::new(), 7);
        xbox.device_id = "xbox".to_string();
        xbox.source_timestamp_ns = 1_500;
        let mut latest_nav = record(vec![0.0, 0.0, -1.0], Vec::new(), 2);
        latest_nav.device_id = "nav".to_string();
        latest_nav.source_timestamp_ns = 2_000;

        let records = coalesce_latest_hid_records_by_device(vec![old_nav, xbox, latest_nav]);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].device_id, "xbox");
        assert_eq!(records[0].sequence, 7);
        assert_eq!(records[1].device_id, "nav");
        assert_eq!(records[1].sequence, 2);
        assert_eq!(records[1].axes[0], 0.0);
    }

    fn edge(epoch: u64, sequence: u64, button: &str, pressed: bool) -> HidButtonEdge {
        HidButtonEdge { epoch, device_id: "pad".into(), edge_sequence: sequence, button: button.into(), pressed }
    }

    #[test]
    fn ordered_edges_preserve_quick_tap_and_dedupe_reorder() {
        let mut cursor = HidSemanticCursor::default();
        assert!(cursor.accept_state(7, 1));
        assert!(cursor.push_edge(edge(7, 2, "a", false)).is_empty());
        let ready = cursor.push_edge(edge(7, 1, "a", true));
        assert_eq!(ready.iter().map(|edge| (edge.pressed, edge.edge_sequence)).collect::<Vec<_>>(), vec![(true, 1), (false, 2)]);
        assert!(cursor.push_edge(edge(7, 1, "a", true)).is_empty());
    }

    #[test]
    fn latest_state_wins_without_wall_clock_ordering() {
        let mut cursor = HidSemanticCursor::default();
        assert!(cursor.accept_state(9, 4));
        assert!(!cursor.accept_state(9, 3));
        assert!(cursor.accept_state(9, 5)); // source wall clock may move backwards; it is irrelevant here.
    }

    #[test]
    fn epoch_reset_rejects_stale_edges() {
        let mut cursor = HidSemanticCursor::default();
        assert!(cursor.accept_state(10, 8));
        assert!(cursor.accept_state(11, 1));
        assert!(cursor.push_edge(edge(10, 9, "a", true)).is_empty());
        assert_eq!(cursor.push_edge(edge(11, 1, "b", true)).len(), 1);
        assert!(!cursor.accept_state(10, 99));
    }

    #[test]
    fn coalesces_by_source_timestamp_not_arrival_order() {
        let mut newer_nav = record(vec![0.0, 1.0, -1.0], Vec::new(), 10);
        newer_nav.device_id = "nav".to_string();
        newer_nav.source_timestamp_ns = 2_000;
        let mut older_nav = record(vec![-1.0, 0.0, -1.0], Vec::new(), 9);
        older_nav.device_id = "nav".to_string();
        older_nav.source_timestamp_ns = 1_000;

        let records = coalesce_latest_hid_records_by_device(vec![newer_nav, older_nav]);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].sequence, 10);
        assert_eq!(records[0].axes[1], 1.0);
    }

    #[test]
    fn maps_nav_stick_and_buttons_to_xinput_shape() {
        let state = map_record_to_virtual_pad(
            &record(
                vec![1.0, -1.0, -1.0],
                vec!["cross", "circle", "l1", "l3", "up"],
                7,
            ),
            &default_mapping(),
        );

        assert_eq!(state.left_x, 32767);
        assert_eq!(state.left_y, 32767);
        assert_eq!(state.left_trigger, 0);
        assert_eq!(state.right_trigger, 0);
        assert!(state.buttons.contains(&"cross".to_string()));
        assert!(state.buttons.contains(&"circle".to_string()));
        assert!(state.buttons.contains(&"l1".to_string()));
        assert!(state.buttons.contains(&"l3".to_string()));
        assert!(state.buttons.contains(&"up".to_string()));
    }

    #[test]
    fn remaps_buttons_and_axes_from_mapping() {
        let mut mapping = default_mapping();
        mapping
            .button_map
            .insert("cross".to_string(), "b".to_string());
        mapping.axis_map.insert(
            "leftX".to_string(),
            AxisBinding {
                source: 1,
                invert: false,
                scale: 0.5,
                deadzone: 0.0,
            },
        );

        let state =
            map_record_to_virtual_pad(&record(vec![1.0, 1.0, -1.0], vec!["cross"], 7), &mapping);

        assert_eq!(state.left_x, 16384);
        assert!(state.buttons.contains(&"b".to_string()));
        assert!(!state.buttons.contains(&"cross".to_string()));
    }

    #[test]
    fn default_mapping_deadzones_small_stick_noise() {
        let state = map_record_to_virtual_pad(
            &record(vec![0.05, -0.04, -1.0], Vec::new(), 7),
            &default_mapping(),
        );

        assert_eq!(state.left_x, 0);
        assert_eq!(state.left_y, 0);
        assert_eq!(state.left_trigger, 0);
        assert_eq!(state.right_trigger, 0);
        assert!(state.buttons.is_empty());
    }

    #[test]
    fn pending_learn_maps_next_active_input() {
        let mut mapping = default_mapping();
        mapping.pending_learn = Some(LearnRequest {
            target: "a".to_string(),
            binding_kind: "button".to_string(),
            requested_at: None,
        });
        let (mut node, store_path) = test_node("learn");
        let options = Options::parse(
            ["--store", store_path.to_str().unwrap(), "--dry-run"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let updated = apply_pending_learn(
            &mut node,
            &options,
            &mapping,
            &record(vec![0.0, 0.0], vec!["cross"], 2),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            updated.button_map.get("cross").map(String::as_str),
            Some("a")
        );
        assert!(updated.pending_learn.is_none());
        let persisted = read_desired_mapping(&mut node, &options);
        assert_eq!(
            persisted.button_map.get("cross").map(String::as_str),
            Some("a")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn runtime_state_prefers_recent_frames_over_handshake_churn() {
        let (node, store_path) = test_node("runtime-fresh-frame");
        let options = Options::parse(
            ["--store", store_path.to_str().unwrap(), "--dry-run"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let mut mapping = default_mapping();
        mapping.enabled = true;
        let stream = create_rudp_stream(None, Some("127.0.0.1:9".to_string()), false).unwrap();

        let state = SleipnirRuntimeState::from_runtime(
            &options,
            Some("nav"),
            &node,
            stream.as_ref(),
            None,
            0,
            &mapping,
            None,
        );

        assert_eq!(state.stream_state, "connected");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn surface_exposes_input_latency_and_axis_classification() {
        let mapping = default_mapping();
        let state = SleipnirRuntimeState {
            provider_id: "sleipnir.input-mirror.test".to_string(),
            selected_muninn_endpoint: Some("127.0.0.1:17888".to_string()),
            selected_device_filter: Some("nav".to_string()),
            presentation: "xbox360".to_string(),
            virtual_backend: "logging.dry-run".to_string(),
            stream_state: "connected".to_string(),
            last_device_id: Some("nav".to_string()),
            last_device_kind: Some("ps3-navigation".to_string()),
            last_sequence: Some(42),
            last_frame_age_ms: Some(3),
            last_input_latency: Some(SleipnirInputLatencySnapshot {
                device_id: "nav".to_string(),
                sequence: 42,
                source_to_arrival_ms: Some(2),
                arrival_to_buffer_ms: 3,
                buffer_to_axis_ms: 1,
                axis_to_hid_ms: 1,
                source_to_hid_ms: Some(7),
                total_observed_ms: 5,
                axis_summary: vec!["leftX<-axis0 value=0.500".to_string()],
                emitted: true,
            }),
            ignored_stream_frames: 0,
            available_devices: Vec::new(),
            axis_map: mapping.axis_map,
            button_map: mapping.button_map,
            pending_learn: None,
            updated_at: "unix-1".to_string(),
        };

        let surface = serde_json::to_string(&sleipnir_surface_document(&state)).unwrap();

        assert!(surface.contains("Input Latency"));
        assert!(surface.contains("arrival to buffer: 3 ms"));
        assert!(surface.contains("buffer to axis classification: 1 ms"));
        assert!(surface.contains("axis to HID emission: 1 ms"));
        assert!(surface.contains("mapped axes: leftX<-axis0 value=0.500"));
    }

    #[test]
    fn available_hid_devices_ignore_move_evidence_streams() {
        let provider = json!({
            "verseId": "starfire.local",
            "inputStreams": [
                {
                    "streamId": "muninn:nightwing:move-evidence",
                    "schema": "muninn.hid_controller_state.v1",
                    "transport": "cultnet.transport.rudp.v0",
                    "address": "198.51.100.75:17888",
                    "devices": [
                        {
                            "deviceId": "nav-ghost",
                            "deviceKind": "ps3-navigation",
                            "sourcePath": "/dev/input/js0"
                        }
                    ]
                },
                {
                    "streamId": "muninn:starfire:hid-controller-state",
                    "schema": "muninn.hid_controller_state.v1",
                    "transport": "cultnet.transport.rudp.v0",
                    "address": "198.51.100.66:17888",
                    "devices": [
                        {
                            "deviceId": "nav-windows-psnav-0",
                            "deviceKind": "ps3-navigation",
                            "sourcePath": "windows-psmove://nav"
                        }
                    ]
                }
            ]
        });

        let devices = available_hid_devices_from_value(&provider);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "nav-windows-psnav-0");
        assert_eq!(devices[0].host_id, "starfire");
        assert_eq!(devices[0].endpoint, "198.51.100.66:17888");
    }

    #[test]
    fn discovers_hid_endpoint_from_odin_provider_routes() {
        let (mut node, store_path) = test_node("hid-endpoint-routes");
        node.put(
            "muninn.telemetry.starfire",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.starfire",
                    "endpoints": [
                        {
                            "address": "C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc",
                            "transport": "cultcache-store"
                        },
                        {
                            "address": "198.51.100.66:17888",
                            "channel": "latest",
                            "connectionId": 1836384261,
                            "role": "muninn.hid_controller_state",
                            "schema": "muninn.hid_controller_state.v1",
                            "transport": "cultnet.transport.rudp.v0"
                        }
                    ],
                    "routes": [
                        {
                            "address": "C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc",
                            "transport": "cultcache-store"
                        }
                    ]
                }),
            },
        )
        .unwrap();

        assert_eq!(
            discover_muninn_hid_endpoint(
                &node,
                Some("nav-windows-psnav-0"),
                Some("starfire:nav-windows-psnav-0:hid-controller-state"),
                false,
            )
            .as_deref(),
            Some("198.51.100.66:17888")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn discovers_available_hid_devices_from_typed_muninn_state_records() {
        let (mut node, store_path) = test_node("hid-record-discovery");
        node.put(
            "muninn.telemetry.starfire",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.starfire",
                    "endpoints": [{
                        "address": "198.51.100.66:17888",
                        "schema": "muninn.hid_controller_state.v1",
                        "transport": "cultnet.transport.rudp.v0"
                    }]
                }),
            },
        )
        .unwrap();
        node.put(
            "starfire:nav-windows-psnav-0:hid-controller-state",
            &MuninnHidControllerStateRecord {
                stream_id: "starfire:nav-windows-psnav-0:hid-controller-state".to_string(),
                host_id: "starfire".to_string(),
                device_id: "nav-windows-psnav-0".to_string(),
                device_kind: "ps3-navigation".to_string(),
                sequence: 1,
                source_timestamp_ns: 0,
                axes: vec![0.0, 0.0, 0.0],
                buttons: Vec::new(),
                battery01: f32::NAN,
                observed_at: "unix:0".to_string(),
                source_path: "windows-psmove://nav".to_string(),
            },
        )
        .unwrap();

        let devices = discover_available_hid_devices(&node);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "nav-windows-psnav-0");
        assert_eq!(devices[0].host_id, "starfire");
        assert_eq!(
            devices[0].stream_id,
            "starfire:nav-windows-psnav-0:hid-controller-state"
        );
        assert_eq!(devices[0].endpoint, "198.51.100.66:17888");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn normalizes_tuple_shaped_hid_state_payloads_for_discovery() {
        let (mut node, store_path) = test_node("hid-tuple-discovery");
        node.put(
            "muninn.telemetry.raven",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.raven",
                    "endpoints": [{
                        "address": "198.51.100.84:17888",
                        "schema": "muninn.hid_controller_state.v1",
                        "transport": "cultnet.transport.rudp.v0"
                    }]
                }),
            },
        )
        .unwrap();
        let device = available_hid_device_from_hid_value(
            &node,
            &json!([
                "raven:xbox-raven:hid-controller-state",
                "raven",
                "xbox-raven",
                "xbox-controller",
                42,
                0,
                [],
                [],
                null,
                "unix:0",
                "xinput://raven/xbox"
            ]),
        )
        .unwrap();

        assert_eq!(device.device_id, "xbox-raven");
        assert_eq!(device.host_id, "raven");
        assert_eq!(device.endpoint, "198.51.100.84:17888");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn available_hid_device_upsert_deduplicates_endpoint_variants() {
        let mut devices = Vec::new();
        upsert_available_hid_device(
            &mut devices,
            AvailableHidDevice {
                device_id: "nav-windows-psnav-0".to_string(),
                device_kind: "ps3-navigation".to_string(),
                source_path: "windows-psmove://nav".to_string(),
                stream_id: "starfire:nav-windows-psnav-0:hid-controller-state".to_string(),
                host_id: "starfire".to_string(),
                endpoint: "discovering".to_string(),
            },
        );
        upsert_available_hid_device(
            &mut devices,
            AvailableHidDevice {
                device_id: "nav-windows-psnav-0".to_string(),
                device_kind: "ps3-navigation".to_string(),
                source_path: "windows-psmove://nav".to_string(),
                stream_id: "starfire:nav-windows-psnav-0:hid-controller-state".to_string(),
                host_id: "starfire".to_string(),
                endpoint: "198.51.100.66:17888".to_string(),
            },
        );
        upsert_available_hid_device(
            &mut devices,
            AvailableHidDevice {
                device_id: "nav-windows-psnav-0".to_string(),
                device_kind: "ps3-navigation".to_string(),
                source_path: "windows-psmove://nav".to_string(),
                stream_id: "muninn:starfire:hid-controller-state".to_string(),
                host_id: "starfire".to_string(),
                endpoint: "198.51.100.66:17888".to_string(),
            },
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].endpoint, "198.51.100.66:17888");
        assert_eq!(
            devices[0].stream_id,
            "starfire:nav-windows-psnav-0:hid-controller-state"
        );
    }

    #[test]
    fn endpoint_discovery_does_not_borrow_other_hosts_from_surfaces() {
        let (mut node, store_path) = test_node("hid-no-surface-leak");
        node.put(
            "sleipnir.input-mirror.raven",
            &EveSurfaceStateRecord {
                provider_id: "sleipnir.input-mirror.raven".to_string(),
                title: "Sleipnir Raven".to_string(),
                version: 1,
                updated_at: "unix:0".to_string(),
                surface: json!({
                    "id": "sleipnir.input-mirror.raven.surface",
                    "root": {
                        "kind": "dashboard",
                        "children": [{
                            "kind": "text",
                            "props": {
                                "text": "raven sees nightwing endpoint 198.51.100.75:17888"
                            }
                        }]
                    }
                }),
            },
        )
        .unwrap();

        assert_eq!(
            discover_muninn_hid_endpoint(
                &node,
                Some("xbox-raven"),
                Some("raven:xbox-raven:hid-controller-state"),
                false,
            ),
            None
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn stream_id_host_hint_filters_provider_routes() {
        let (mut node, store_path) = test_node("hid-host-hint");
        node.put(
            "muninn.telemetry.nightwing",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.nightwing",
                    "verseId": "nightwing.local",
                    "endpoints": [{
                        "address": "198.51.100.75:17888",
                        "schema": "muninn.hid_controller_state.v1",
                        "transport": "cultnet.transport.rudp.v0"
                    }]
                }),
            },
        )
        .unwrap();
        node.put(
            "muninn.telemetry.starfire",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.starfire",
                    "verseId": "starfire.local",
                    "endpoints": [{
                        "address": "198.51.100.66:17888",
                        "schema": "muninn.hid_controller_state.v1",
                        "transport": "cultnet.transport.rudp.v0"
                    }]
                }),
            },
        )
        .unwrap();
        let hint = stream_host_hint(Some("starfire:nav-windows-psnav-0:hid-controller-state"));

        assert_eq!(hint.as_deref(), Some("starfire"));
        assert_eq!(
            discover_muninn_hid_endpoint(
                &node,
                Some("nav-windows-psnav-0"),
                Some("starfire:nav-windows-psnav-0:hid-controller-state"),
                false,
            )
            .as_deref(),
            Some("198.51.100.66:17888")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn selected_provider_device_resolves_input_stream_endpoint() {
        let (mut node, store_path) = test_node("hid-provider-device-endpoint");
        node.put(
            "muninn.telemetry.starfire",
            &EveProviderAdvertisementRecord {
                value: json!({
                    "providerId": "muninn.telemetry.starfire",
                    "verseId": "starfire.local",
                    "inputStreams": [{
                        "address": "198.51.100.66:17888",
                        "channel": "latest",
                        "connectionId": 1836384261,
                        "schema": "muninn.hid_controller_state.v1",
                        "streamId": "muninn:starfire:hid-controller-state",
                        "transport": "cultnet.transport.rudp.v0",
                        "devices": [{
                            "deviceId": "nav-windows-psnav-0",
                            "deviceKind": "ps3-navigation",
                            "sourcePath": "windows-psmove://nav"
                        }]
                    }]
                }),
            },
        )
        .unwrap();

        assert_eq!(
            discover_muninn_hid_endpoint(
                &node,
                Some("nav-windows-psnav-0"),
                Some("muninn:starfire:hid-controller-state"),
                false,
            )
            .as_deref(),
            Some("198.51.100.66:17888")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn desired_mapping_ignores_transport_endpoint_shortcuts() {
        let (mut node, store_path) = test_node("mapping");
        let options = Options::parse(
            ["--store", store_path.to_str().unwrap()]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        node.put(
            &sleipnir_provider_id(&options),
            &SleipnirInputMappingRecord {
                provider_id: sleipnir_provider_id(&options),
                enabled: true,
                device_filter: "nav-windows-psnav-0".to_string(),
                stream_id: String::new(),
                presentation: "xbox360".to_string(),
                axis_map: serde_json::Value::Null,
                button_map: serde_json::Value::Null,
                pending_learn: json!({
                    "muninnRudp": "198.51.100.66:17888"
                }),
                updated_at: timestamp(),
                source: "test".to_string(),
            },
        )
        .unwrap();

        let mapping = read_desired_mapping(&mut node, &options);

        assert_eq!(
            mapping.device_filter.as_deref(),
            Some("nav-windows-psnav-0")
        );
        assert_eq!(mapping.stream_id, None);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn desired_mapping_reads_newest_odin_discovery_mapping() {
        let (mut local_node, local_store_path) = test_node("mapping-local");
        let (mut discovery_node, discovery_store_path) = test_node("mapping-discovery");
        let options = Options::parse(
            ["--store", local_store_path.to_str().unwrap()]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let provider_id = sleipnir_provider_id(&options);
        local_node
            .put(
                &provider_id,
                &SleipnirInputMappingRecord {
                    provider_id: provider_id.clone(),
                    enabled: true,
                    device_filter: "local-old".to_string(),
                    stream_id: String::new(),
                    presentation: "xbox360".to_string(),
                    axis_map: serde_json::Value::Null,
                    button_map: serde_json::Value::Null,
                    pending_learn: serde_json::Value::Null,
                    updated_at: "2026-07-04T00:00:00.000Z".to_string(),
                    source: "local".to_string(),
                },
            )
            .unwrap();
        discovery_node
            .put(
                &provider_id,
                &SleipnirInputMappingRecord {
                    provider_id: provider_id.clone(),
                    enabled: true,
                    device_filter: "odin-new".to_string(),
                    stream_id: String::new(),
                    presentation: "xbox360".to_string(),
                    axis_map: serde_json::Value::Null,
                    button_map: serde_json::Value::Null,
                    pending_learn: serde_json::Value::Null,
                    updated_at: "2026-07-04T00:00:01.000Z".to_string(),
                    source: "odin".to_string(),
                },
            )
            .unwrap();

        let mapping =
            read_desired_mapping_from_sources(&mut local_node, Some(&mut discovery_node), &options);

        assert_eq!(mapping.device_filter.as_deref(), Some("odin-new"));
        let _ = fs::remove_file(local_store_path);
        let _ = fs::remove_file(discovery_store_path);
    }

    #[test]
    fn rejects_removed_command_rudp_bind() {
        let error = Options::parse(
            ["--command-rudp-bind", "127.0.0.1:17889"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--command-rudp-bind has been removed"));
    }

    #[test]
    fn accepts_odin_cultmesh_uri() {
        let options = Options::parse(
            [
                "--odin-cultmesh-uri",
                "cultmesh://odin/rendezvous/provider-catalog",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.odin_cultmesh_uri.as_deref(),
            Some("cultmesh://odin/rendezvous/provider-catalog")
        );
    }

    #[test]
    fn rejects_removed_odin_cultmesh_rudp() {
        let error = Options::parse(
            ["--odin-cultmesh-rudp", "127.0.0.1:17871"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--odin-cultmesh-rudp has been removed"));
    }

    #[cfg(windows)]
    #[test]
    fn xbox_face_button_names_stay_distinct() {
        assert_eq!(xbutton_mask("a"), vigem_client::XButtons::A);
        assert_eq!(xbutton_mask("b"), vigem_client::XButtons::B);
        assert_eq!(xbutton_mask("x"), vigem_client::XButtons::X);
        assert_eq!(xbutton_mask("y"), vigem_client::XButtons::Y);
        assert_eq!(xbutton_mask("cross"), vigem_client::XButtons::A);
        assert_eq!(xbutton_mask("square"), vigem_client::XButtons::X);
    }
}
