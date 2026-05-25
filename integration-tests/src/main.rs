use std::path::Path;
use std::process::Command;
use std::time::Duration;

use color_eyre::eyre::{Result, eyre};

fn main() -> Result<()> {
    color_eyre::install()?;

    // build the main image
    docker_build("development/Dockerfile.debug", ".", "f2", "debug")?;

    // build the supporting images
    docker_build(
        "development/servers/echo/Dockerfile.single",
        "development/servers/echo",
        "echo",
        "single",
    )?;

    docker_build(
        "development/servers/echo/Dockerfile.double",
        "development/servers/echo",
        "echo",
        "double",
    )?;

    docker_build(
        "development/servers/volumes/Dockerfile",
        "development/servers/volumes",
        "volumes",
        "latest",
    )?;

    // create the internal network
    create_internal_network()?;

    // run the main container
    let volumes = vec![
        ("./development", "/development"),
        ("/var/run/docker.sock", "/var/run/docker.sock"),
    ];

    docker_run("f2", "debug", &volumes, "/development/volumes-config.yaml")?;

    // give the container a moment to start up
    std::thread::sleep(Duration::from_secs(1));

    // check we can get a response from it
    let mut response = ureq::get("http://localhost:3000").call()?;
    let response_text = response.body_mut().read_to_string()?;

    let expected = std::fs::read_to_string("development/volumes-configuration.json")?;

    if response_text != expected {
        return Err(eyre!(
            "Received unexpected response from container: {response_text}"
        ));
    }

    println!("✅ Successfully received correct response from container");

    // remove the running containers
    docker_remove_running_containers()?;

    // start the next test
    docker_run("f2", "debug", &volumes, "/development/echo-single-config.yaml")?;

    // give the container a moment to start up
    std::thread::sleep(Duration::from_secs(1));

    // check we can get a response from it
    let mut response = ureq::get("http://localhost:3000/foobar").call()?;
    let response_text = response.body_mut().read_to_string()?;

    if response_text != "Echo foobar" {
        return Err(eyre!(
            "Received unexpected response from container: {response_text}"
        ));
    }

    println!("✅ Successfully received correct response from container");

    // roll to a new version
    swap("./development/echo-single-config.yaml", "./development/echo-double-config.yaml")?;

    // force a reconciliation
    let mut response = ureq::put("http://localhost:3000/reconcile").send_empty()?;

    if !response.status().is_success() {
        return Err(eyre!(
            "Failed to trigger reconciliation: {}",
            response.body_mut().read_to_string()?
        ));
    }

    // give the container a moment to reconcile
    std::thread::sleep(Duration::from_secs(1));

    // check we can get a response from it
    let mut response = ureq::get("http://localhost:3000/foobar").call()?;
    let response_text = response.body_mut().read_to_string()?;

    if response_text != "Echo echo foobar" {
        return Err(eyre!(
            "Received unexpected response from container: {response_text}"
        ));
    }

    docker_remove_running_containers()?;

    swap("./development/echo-single-config.yaml", "./development/echo-double-config.yaml")?;

    Ok(())
}

fn docker_build<D: AsRef<Path>, C: AsRef<Path>>(
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

fn create_internal_network() -> Result<()> {
    // check whether the network already exists
    let output = Command::new("docker")
        .arg("network")
        .arg("ls")
        .arg("--filter")
        .arg("name=^internal$")
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

    if existing_networks
        .lines()
        .any(|line| line.trim() == "internal")
    {
        println!("✅ Docker network 'internal' already exists");
        return Ok(());
    }

    // create the network
    let output = Command::new("docker")
        .arg("network")
        .arg("create")
        .arg("internal")
        .output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to create Docker network: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    println!("✅ Successfully created Docker network: internal");

    Ok(())
}

fn docker_run(
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

fn docker_remove_running_containers() -> Result<()> {
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

fn swap(path1: &str, path2: &str) -> Result<()> {
    std::fs::rename(path1, format!("{}.tmp", path1))?;
    std::fs::rename(path2, path1)?;
    std::fs::rename(format!("{}.tmp", path1), path2)?;

    println!("✅ Successfully swapped {path1} and {path2}");

    Ok(())
}
