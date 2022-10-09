# wssocks for shuttle.rs

## Features

socks5 proxy over websocket host on shuttle.rs

## Quick Start

### Deploy

```sh
cargo install cargo-shuttle
cargo shuttle login
cargo shuttle deploy
```

### Client

<https://github.com/ginuerzh/gost>

```sh
gost -L=socks5://:1080 -F=socks5+wss://myapp.shuttleapp.rs:443
```

## Reference

- <https://github.com/ginuerzh/gost>
- <https://github.com/kingluo/tokio-socks5.git>
- <https://github.com/suikammd/rust-proxy.git>

## License

MIT License
