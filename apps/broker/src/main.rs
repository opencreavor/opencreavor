#[tokio::main]
async fn main() -> anyhow::Result<()> {
    creavor_broker::run().await
}
