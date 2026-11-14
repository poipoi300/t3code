//! TERM-2: session wiring (attach, resume, write, resize). Its connected behaviour needs a live
//! session, so it is tested where it lives: the frame-coalesced pump in
//! `src/screens/terminal/session.rs` (counted work) and the client service against the fake
//! server in `t3-client/tests/client_services_terminal.rs`. The disconnected drawer is in `term_1`.
