# tourniquet-tonic

[Tourniquet](https://docs.rs/tourniquet) integration with the [celery](https://docs.rs/celery)
library.

## Example

```rust
#
#
let rr = RoundRobin::new(
    vec!["https://api01", "https://api02"],
    TonicConnector::default(),
);

rr.run(|channel| async move {
    grpc::greeting_client::GreetingClient::new(channel.as_ref().clone())
        .hello(grpc::Message::default())
        .await?;
    Ok(())
}).await?;
```

License: MIT
