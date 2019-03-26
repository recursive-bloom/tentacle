use env_logger;
use futures::{oneshot, prelude::*, sync::oneshot::Sender};
use log::info;
use std::collections::HashMap;
use std::{
    str,
    time::{Duration, Instant},
};
use tentacle::{
    builder::{MetaBuilder, ServiceBuilder},
    context::{ProtocolContext, ProtocolContextMutRef, ServiceContext},
    secio::SecioKeyPair,
    service::{DialProtocol, ProtocolHandle, ProtocolMeta, Service, ServiceError, ServiceEvent},
    traits::{ServiceHandle, ServiceProtocol},
    ProtocolId, SessionId,
};
use tokio::timer::{Delay, Error, Interval};

fn create_meta(id: ProtocolId) -> ProtocolMeta {
    MetaBuilder::new()
        .id(id)
        .service_handle(move || {
            // All protocol use the same handle.
            // This is just an example. In the actual environment, this should be a different handle.
            let handle = Box::new(PHandle {
                proto_id: id,
                count: 0,
                connected_session_ids: Vec::new(),
                clear_handle: HashMap::new(),
            });
            ProtocolHandle::Callback(handle)
        })
        .build()
}

#[derive(Default)]
struct PHandle {
    proto_id: ProtocolId,
    count: usize,
    connected_session_ids: Vec<SessionId>,
    clear_handle: HashMap<SessionId, Sender<()>>,
}

impl ServiceProtocol for PHandle {
    fn init(&mut self, control: &mut ProtocolContext) {
        if self.proto_id == 0 {
            control.set_service_notify(0, Duration::from_secs(5), 3);
        }
    }

    fn connected(&mut self, mut control: ProtocolContextMutRef, version: &str) {
        let session = control.session;
        self.connected_session_ids.push(session.id);
        info!(
            "proto id [{}] open on session [{}], address: [{}], type: [{:?}], version: {}",
            self.proto_id, session.id, session.address, session.ty, version
        );
        info!("connected sessions are: {:?}", self.connected_session_ids);

        if self.proto_id != 1 {
            return;
        }

        // Register a scheduled task to send data to the remote peer.
        // Clear the task via channel when disconnected
        let (sender, mut receiver) = oneshot();
        self.clear_handle.insert(session.id, sender);
        let session_id = session.id;
        let mut interval_sender = control.control().clone();
        let interval_task = Interval::new(Instant::now(), Duration::from_secs(5))
            .for_each(move |_| {
                let _ = interval_sender.send_message(
                    session_id,
                    1,
                    b"I am a interval message".to_vec(),
                );
                if let Ok(Async::Ready(_)) = receiver.poll() {
                    Err(Error::shutdown())
                } else {
                    Ok(())
                }
            })
            .map_err(|err| info!("{}", err));
        control.future_task(interval_task);
    }

    fn disconnected(&mut self, control: ProtocolContextMutRef) {
        let new_list = self
            .connected_session_ids
            .iter()
            .filter(|&id| id != &control.session.id)
            .cloned()
            .collect();
        self.connected_session_ids = new_list;

        if let Some(handle) = self.clear_handle.remove(&control.session.id) {
            let _ = handle.send(());
        }

        info!(
            "proto id [{}] close on session [{}]",
            self.proto_id, control.session.id
        );
    }

    fn received(&mut self, env: ProtocolContextMutRef, data: bytes::Bytes) {
        self.count += 1;
        info!(
            "received from [{}]: proto [{}] data {:?}, message count: {}",
            env.session.id,
            self.proto_id,
            str::from_utf8(data.as_ref()).unwrap(),
            self.count
        );
    }

    fn notify(&mut self, _control: &mut ProtocolContext, token: u64) {
        info!("proto [{}] received notify token: {}", self.proto_id, token);
    }
}

struct SHandle;

impl ServiceHandle for SHandle {
    fn handle_error(&mut self, _env: &mut ServiceContext, error: ServiceError) {
        info!("service error: {:?}", error);
    }
    fn handle_event(&mut self, env: &mut ServiceContext, event: ServiceEvent) {
        info!("service event: {:?}", event);
        if let ServiceEvent::SessionOpen { .. } = event {
            let mut delay_sender = env.control().clone();

            let delay_task = Delay::new(Instant::now() + Duration::from_secs(3))
                .and_then(move |_| {
                    let _ =
                        delay_sender.filter_broadcast(None, 0, b"I am a delayed message".to_vec());
                    Ok(())
                })
                .map_err(|err| info!("{}", err));

            env.future_task(Box::new(delay_task));
        }
    }
}

fn main() {
    env_logger::init();

    if std::env::args().nth(1) == Some("server".to_string()) {
        info!("Starting server ......");
        server();
    } else {
        info!("Starting client ......");
        client();
    }
}

fn create_server() -> Service<SHandle> {
    ServiceBuilder::default()
        .insert_protocol(create_meta(0))
        .insert_protocol(create_meta(1))
        .key_pair(SecioKeyPair::secp256k1_generated())
        .build(SHandle)
}

/// Proto 0 open success
/// Proto 1 open success
/// Proto 2 open failure
///
/// Because server only supports 0,1
fn create_client() -> Service<SHandle> {
    ServiceBuilder::default()
        .insert_protocol(create_meta(0))
        .insert_protocol(create_meta(1))
        .insert_protocol(create_meta(2))
        .key_pair(SecioKeyPair::secp256k1_generated())
        .build(SHandle)
}

fn server() {
    let mut service = create_server();
    let _ = service.listen("/ip4/127.0.0.1/tcp/1337".parse().unwrap());

    tokio::run(service.for_each(|_| Ok(())))
}

fn client() {
    let mut service = create_client();
    service
        .dial(
            "/dns4/localhost/tcp/1337".parse().unwrap(),
            DialProtocol::All,
        )
        .unwrap();
    let _ = service.listen("/ip4/127.0.0.1/tcp/1337".parse().unwrap());

    tokio::run(service.for_each(|_| Ok(())))
}
