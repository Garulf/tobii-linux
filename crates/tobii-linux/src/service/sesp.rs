//! Service side of SESP: head pose for the game DLL's tet-tcp devices through the pipe relay.
//!
//! The bridge owns `\\.\pipe\streamengineservices` and relays each connection like a PRP
//! pipe (docs/prp-bridge.md). For every relayed connection this module answers the SESP
//! hello with a slot index, pairs the command channel with the subscription channel named
//! in the initialize request, and pushes the head-pose hub's samples as SESP head-pose
//! messages once the client starts head pose. Messages are those of ses_windows.dll
//! (tobii_service::sesp). The game layer's own SESP client sends usage statistics, which
//! are acknowledged and dropped. Requests it does not serve get the service's own "invalid
//! operation" signal. docs/sesp.md has the evidence.
//!
//! Head positions are sent as the inverse of the legacy module's transform for the device
//! display area, so the module turns them back into the positions the PRP route delivers
//! (tobii_service::sesp_service); without compensation they are sent unchanged, as
//! ses_windows.dll does.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use tobii_headpose::pipeline::HeadPoseSample;
use tobii_service::sesp::{
    self as codec, ERROR_NONE, ERROR_NOT_FOUND, ERROR_NOT_SUPPORTED, FrameDecoder, REQUEST_HEADPOSE_START,
    REQUEST_HEADPOSE_STOP, REQUEST_INITIALIZE, REQUEST_NOTIFY_STREAM_STATUS, REQUEST_SEND_STATISTICS,
    RESPONSE_HEADPOSE_START, RESPONSE_HEADPOSE_STOP, RESPONSE_SEND_STATISTICS, SIGNAL_INVALID_OPERATION,
    SUBSCRIPTION_CHANNEL,
};
use tobii_service::sesp_service::{self as decisions, CAPABILITIES, LegacyPositionTransform, MAX_PENDING_BYTES};

use super::hub::Hub;
use super::link::{Frames, Link, Outcome};
use super::{GameTime, PrpService};

/// One relayed SESP connection: a command or a subscription channel.
#[derive(Debug)]
struct SespClient {
    slot: u32,
    link: Link,
    pipe: String,
    channel: Cell<u32>,
    peer: RefCell<Option<Rc<SespClient>>>,
    sequence: Cell<u32>,
    head_pose: Cell<Option<u64>>,
}

impl SespClient {
    fn send(&self, message: &[u8]) {
        if !self.link.is_closing() {
            log::debug!("{} -> {}", self.pipe, super::link::hex(message));
            self.link.message(message);
        }
    }
}

/// Slot table and head-pose source shared by every SESP connection.
///
/// SESP has no timesync: the module hands the head-pose timestamps to the game unchanged,
/// so they are mapped onto the game's QueryPerformanceCounter microseconds.
#[derive(Debug)]
pub struct SespServer {
    hub: Rc<Hub<HeadPoseSample>>,
    game_time: GameTime,
    compensate_position: bool,
    slots: Cell<u32>,
    clients: RefCell<BTreeMap<u32, Rc<SespClient>>>,
    pub sent: Rc<Cell<u64>>,
    pub dropped: Rc<Cell<u64>>,
}

impl SespServer {
    pub fn new(hub: Rc<Hub<HeadPoseSample>>, game_time: GameTime, compensate_position: bool) -> Self {
        Self {
            hub,
            game_time,
            compensate_position,
            slots: Cell::new(1),
            clients: RefCell::new(BTreeMap::new()),
            sent: Rc::default(),
            dropped: Rc::default(),
        }
    }

    pub fn clients(&self) -> usize {
        self.clients.borrow().len()
    }

    fn position_transform(&self, service: &PrpService) -> Option<LegacyPositionTransform> {
        if !self.compensate_position {
            return None;
        }
        let [top_left, top_right, bottom_left] = service.legacy_display_area()?;
        Some(LegacyPositionTransform::from_display_area(top_left, top_right, bottom_left))
    }

    pub async fn serve(&self, service: &PrpService, link: Link, frames: &mut Frames, client_pid: u32) -> Outcome {
        let slot = self.slots.get();
        self.slots.set(slot + 1);
        let pipe = codec::PIPE_NAME.to_string();
        let client = Rc::new(SespClient {
            slot,
            link,
            pipe: pipe.clone(),
            channel: Cell::new(0),
            peer: RefCell::new(None),
            sequence: Cell::new(1),
            head_pose: Cell::new(None),
        });
        let outcome = async {
            let hello = codec::decode_hello(&frames.message(&pipe).await?)?;
            client.channel.set(hello.channel);
            client.link.frame(
                tobii_service::prp_service::FRAME_REPLY_PIPE,
                &tobii_service::prp_service::encode_names(&[hello.reply_pipe.as_str()])?,
            );
            client.send(&codec::encode_slot(slot));
            log::info!("SESP channel {:#x} from pid {client_pid} is slot {slot}", hello.channel);
            self.clients.borrow_mut().insert(slot, Rc::clone(&client));
            let mut decoder = FrameDecoder::default();
            loop {
                let data = frames.message(&pipe).await?;
                for buffer in decoder.feed(&data)? {
                    let message = codec::decode(&buffer)?;
                    self.handle(service, &client, &message)?;
                }
            }
        }
        .await;
        self.clients.borrow_mut().remove(&slot);
        self.stop_head_pose(&client);
        if let Some(peer) = client.peer.borrow_mut().take() {
            self.stop_head_pose(&peer);
            peer.link.close();
            peer.peer.borrow_mut().take();
        }
        outcome
    }

    fn handle(&self, service: &PrpService, client: &Rc<SespClient>, message: &codec::Message) -> Outcome {
        let sequence = message.sequence;
        match message.kind {
            REQUEST_INITIALIZE => self.initialize(client, message)?,
            REQUEST_HEADPOSE_START => {
                let error = self.start_head_pose(service, client);
                client.send(&codec::encode_error_response(sequence, RESPONSE_HEADPOSE_START, i64::from(error)));
            }
            REQUEST_HEADPOSE_STOP => {
                if let Some(peer) = client.peer.borrow().as_ref() {
                    self.stop_head_pose(peer);
                }
                client.send(&codec::encode_error_response(sequence, RESPONSE_HEADPOSE_STOP, i64::from(ERROR_NONE)));
                log::info!("SESP head pose stopped for slot {}", client.slot);
            }
            REQUEST_NOTIFY_STREAM_STATUS => client.send(&codec::encode_stream_status_response(sequence)),
            REQUEST_SEND_STATISTICS => {
                client.send(&codec::encode_error_response(sequence, RESPONSE_SEND_STATISTICS, i64::from(ERROR_NONE)));
            }
            kind => {
                log::info!("SESP request type {kind:#x} on slot {} not served", client.slot);
                client.send(&codec::encode_error_response(
                    sequence,
                    SIGNAL_INVALID_OPERATION,
                    i64::from(ERROR_NOT_SUPPORTED),
                ));
            }
        }
        Ok(())
    }

    /// Pair with the subscription slot named in the request; ses_windows answers 7 without it.
    fn initialize(&self, client: &Rc<SespClient>, message: &codec::Message) -> Outcome {
        let request = codec::initialize_request(message.kind, message.payload.as_ref())?;
        let subscription = self.clients.borrow().get(&request.subscription_id).cloned();
        let Some(subscription) = subscription.filter(|s| s.channel.get() == SUBSCRIPTION_CHANNEL) else {
            log::info!(
                "SESP initialize for {} names unknown subscription slot {}",
                request.device_url,
                request.subscription_id
            );
            client.send(&codec::encode_initialize_response(message.sequence, i64::from(ERROR_NOT_FOUND), &[]));
            return Ok(());
        };
        *client.peer.borrow_mut() = Some(Rc::clone(&subscription));
        *subscription.peer.borrow_mut() = Some(Rc::clone(client));
        log::info!(
            "SESP initialize for {}: command slot {}, subscription slot {}",
            request.device_url,
            client.slot,
            subscription.slot
        );
        client.send(&codec::encode_initialize_response(message.sequence, i64::from(ERROR_NONE), &CAPABILITIES));
        Ok(())
    }

    fn start_head_pose(&self, service: &PrpService, client: &SespClient) -> u32 {
        let Some(peer) = client.peer.borrow().clone() else { return ERROR_NOT_FOUND };
        self.push_head_pose(service, &peer);
        log::info!("SESP head pose started on subscription slot {}", peer.slot);
        ERROR_NONE
    }

    fn push_head_pose(&self, service: &PrpService, client: &Rc<SespClient>) {
        if client.head_pose.get().is_some() {
            return;
        }
        let target = Rc::downgrade(client);
        let (sent, dropped) = (Rc::clone(&self.sent), Rc::clone(&self.dropped));
        let service = service.clone();
        let id = self.hub.subscribe(move |sample: &HeadPoseSample| {
            let Some(client) = target.upgrade() else { return };
            if client.link.pending() > MAX_PENDING_BYTES {
                dropped.set(dropped.get() + 1);
                return;
            }
            let Some(sesp) = service.sesp() else { return };
            let sequence = client.sequence.get();
            client.sequence.set(sequence.wrapping_add(1));
            let pose = decisions::HeadPoseSample {
                valid: sample.valid,
                position_mm: sample.position_mm.map(f64::from),
                rotation_rad: sample.rotation_rad.map(f64::from),
            };
            let transform = sesp.position_transform(&service);
            let time = sesp.game_time.map(sample.timestamp_us);
            client.send(&decisions::head_pose_message(sequence, &pose, time, transform.as_ref()));
            sent.set(sent.get() + 1);
        });
        client.head_pose.set(Some(id));
    }

    fn stop_head_pose(&self, client: &SespClient) {
        if let Some(id) = client.head_pose.take() {
            self.hub.unsubscribe(id);
        }
    }
}
