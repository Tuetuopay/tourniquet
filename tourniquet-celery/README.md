# tourniquet-celery

[Tourniquet](https://docs.rs/tourniquet) integration with the [celery](https://docs.rs/celery)
library.

## Example

```rust
#
#[celery::task]
async fn do_work(work: String) -> TaskResult<()> {
    // Some work
    Ok(())
}

let rr = RoundRobin::new(
    vec!["amqp://rabbit01:5672/".to_owned(), "amqp://rabbit02:5672".to_owned()],
    CeleryConnector { name: "rr", routes: &[("*", "my_route")], ..Default::default() },
);

rr.send_task(|| do_work::new(work.clone())).await.expect("Failed to send task");
```

License: MIT
