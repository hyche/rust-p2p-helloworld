# rust-p2p-helloworld
A simple p2p network written in Rust. Read here for more details https://github.com/massbitprotocol/helloworld-rust-project

# Usage
Compile and run with
```sh
RUST_LOG=info SERVER_ADDRESS=127.0.0.1:3000 cargo run
```

Send JSON-RPC command with curl

```sh
curl -H "Content-Type: application/json" -d '{"jsonrpc": "2.0", "method": "get_pokemon", "params": {"name": "hyhy"}, "id": "1"}' "http://localhost:3000"
```

```sh
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc": "2.0", "method": "create_pokemon", "params": {"name": "hyhy", "color": "pink"}, "id": "1"}' "http://localhost:3000"
```

```sh
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc": "2.0", "method": "update_pokemon", "params": {"name": "hyhy", "color": "green"}, "id": "1"}' "http://localhost:3000"
```
