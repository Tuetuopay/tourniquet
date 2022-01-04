# tourniquet

Easily round-robin between servers providing the same service, automatically reconnecting to the
next server should an error happen.

This library facilitates resiliency to multiple service providers (e.g. servers) by connecting
to the first available service from a list in a round-robin manner. If for some reason any
provider goes down, any attempt to interact with it will reconnect to the next one,
automatically.

Disclaimer: this library is not for load-balancing between a set of providers! It will connect
to _one_ provider, and only use this one provider as long as it is up. Tourniquet is meant for
resiliency and not for load balancing.

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
        vec!["46.16.175.175".parse().unwrap(), "51.161.82.214".parse().unwrap()],
        Conn(6667),
    );

    let hello = rr.run(|sock| async move {
        let mut sock = sock.lock().await;
        let mut buf = [0; 50];
        sock.read_exact(&mut buf).await.map(|_| String::from_utf8(buf.to_vec()).unwrap())
    }).await.unwrap();

    assert!(hello.contains("libera.chat"));
}
```

License: MIT
