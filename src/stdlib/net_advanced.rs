pub fn websocket_connect(url: &str) -> String { format!("ws_connected:{}", url) }\npub fn grpc_call(service: &str) -> String { format!("grpc:{}", service) }
