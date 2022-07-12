mod docker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    docker::create_and_start("alexanderjackson/echo-server", "2046", 5000).await?;

    Ok(())
}
