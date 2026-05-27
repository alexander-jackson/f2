use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, eyre};

pub fn build<D: AsRef<Path>, C: AsRef<Path>>(
    dockerfile: D,
    context: C,
    image: &str,
    version: &str,
) -> Result<()> {
    let tag = format!("{}:{}", image, version);

    let output = Command::new("docker")
        .arg("build")
        .arg("-f")
        .arg(dockerfile.as_ref().as_os_str())
        .arg("-t")
        .arg(&tag)
        .arg(context.as_ref().as_os_str())
        .output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Docker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    println!("✅ Successfully built Docker image: {tag}");

    Ok(())
}

pub fn create_network_if_not_exists(network: &str) -> Result<()> {
    // check whether the network already exists
    let search_pattern = format!("name=^{}$", network);

    let output = Command::new("docker")
        .arg("network")
        .arg("ls")
        .arg("--filter")
        .arg(search_pattern)
        .arg("--format")
        .arg("{{.Name}}")
        .output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to check for existing Docker networks: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let existing_networks = String::from_utf8_lossy(&output.stdout);

    if existing_networks.lines().any(|line| line.trim() == network) {
        println!("✅ Docker network '{network}' already exists");
        return Ok(());
    }

    // create the network
    let output = Command::new("docker")
        .arg("network")
        .arg("create")
        .arg(network)
        .output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to create Docker network: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    println!("✅ Successfully created Docker network: {network}");

    Ok(())
}

pub fn run(
    image: &str,
    version: &str,
    volumes: &[(&'static str, &'static str)],
    configuration_file: &'static str,
) -> Result<String> {
    let tag = format!("{}:{}", image, version);

    let mut command = Command::new("docker");
    command.arg("run").arg("-d").arg("-p").arg("3000:3000");

    for (host_path, container_path) in volumes {
        command
            .arg("--volume")
            .arg(format!("{}:{}", host_path, container_path));
    }

    let output = command
        .arg("--network")
        .arg("internal")
        .arg("--env-file")
        .arg(".env")
        .arg(&tag)
        .arg("--")
        .arg("--config")
        .arg(configuration_file)
        .output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    println!("✅ Successfully started Docker container {container_id} from image {tag}");

    Ok(container_id)
}

pub fn remove_running_containers() -> Result<()> {
    let output = Command::new("docker").arg("ps").arg("-q").output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to list running Docker containers: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let container_ids = String::from_utf8_lossy(&output.stdout);

    for container_id in container_ids.lines() {
        let output = Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(container_id)
            .output()?;

        if !output.status.success() {
            eprintln!(
                "⚠️ Failed to remove Docker container {container_id}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            println!("✅ Successfully removed Docker container {container_id}");
        }
    }

    Ok(())
}
