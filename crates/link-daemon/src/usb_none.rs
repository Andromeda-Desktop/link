//! No USB path on Windows: `StreamLink` carries a descriptor, which Windows sockets are not.

use std::sync::Arc;

use link_core::path::PathSocket;

use crate::hub::HubHandle;

#[expect(clippy::unused_async, reason = "the Unix module's signature")]
pub async fn start(_socket: Arc<PathSocket>, _hub: HubHandle) {}
