//! Effect 4 JSON RPC for native and `wasm32-unknown-unknown` clients and servers.
//! Transport, execution, and monotonic millisecond ticks belong to the caller.

pub mod client;
pub mod codec;
pub mod framing;
pub mod server;
pub mod transport;

pub use client::{CallEvent, CallKind, Client, ClientError, ClientOptions};
pub use codec::{
    Cause, ClientMessage, Exit, FiberId, Frame, Request, RequestId, ServerMessage, True,
};
pub use server::{Server, ServerAction, ServerError, WindowPolicy};
pub use transport::{
    AsyncClient, AsyncServer, Handler, HandlerOutput, ResponseStream, TransportError,
};
