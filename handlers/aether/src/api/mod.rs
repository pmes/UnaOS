// Platform API modules. The DOM/WebIDL bindings live in `crate::js`; these are the input-event type,
// accessibility, the natives under the URL/codec/crypto preludes, and the network APIs (AETHERNET, SR70:
// streaming fetch, WebSocket, EventSource, XMLHttpRequest — their worker threads and JS preludes; the
// task source and natives are `crate::js::net`).
pub mod a11y;
pub mod events;
pub mod eventsource;
pub mod fetch;
pub mod platform;
pub mod uievents;
pub mod websockets;
pub mod xhr;
