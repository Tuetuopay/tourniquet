# tourniquet

Easily round-robin between servers providing the same service, automatically reconnecting to the
next server should an error happen.

## Example

```rust
use async_trait::async_trait;
use std::{io::Error, net::IpAddr};
use tokio::{io::AsyncReadExt, net::TcpStream, sync::Mutex};
use tourniquet::{Connector, RoundRobin};

struct Conn(u16);

#[async_trait]
impl Connector<IpAddr, Mutex<TcpStream>, Error> for Conn {
    async fn connect(&self, src: &IpAddr) -> Result<Mutex<TcpStream>, Error> {
        let Conn(ref port) = self;
        TcpStream::connect((*src, *port)).await.map(Mutex::new)
    }
}

#[tokio::main]
async fn main() {
    let rr = RoundRobin::new(
        vec!["185.30.166.38".parse().unwrap(), "66.110.9.37".parse().unwrap()],
        Conn(6667),
    );

    let hello = rr.run(|sock| async move {
        let mut sock = sock.lock().await;
        let mut buf = [0; 50];
        sock.read_exact(&mut buf).await.map(|_| String::from_utf8(buf.to_vec()).unwrap())
    }).await.unwrap();

    assert!(hello.contains("freenode.net"));
}
```

License: MIT
