use std::time::Duration;

use color_eyre::eyre::{Context, Result, eyre};

mod docker;

fn main() -> Result<()> {
    color_eyre::install()?;

    setup_dependencies().wrap_err("Failed to set up dependencies")?;

    // check that volumes work correctly
    check_volumes_work().wrap_err("Volumes did not work correctly")?;

    // check that args are passed through and override the image CMD correctly
    // (must run before check_rolls_work, whose reconciler prunes all unused images)
    check_args_work().wrap_err("Args did not work correctly")?;

    // check that rolls work correctly
    check_rolls_work().wrap_err("Rolls did not work correctly")?;

    Ok(())
}

/// Builds the necessary Docker images and creates the network for the tests to run.
fn setup_dependencies() -> Result<()> {
    // build the main image
    crate::docker::build("development/Dockerfile.debug", ".", "f2", "debug")?;

    // build the supporting images
    crate::docker::build(
        "development/servers/echo/Dockerfile.single",
        "development/servers/echo",
        "echo",
        "single",
    )?;

    crate::docker::build(
        "development/servers/echo/Dockerfile.double",
        "development/servers/echo",
        "echo",
        "double",
    )?;

    crate::docker::build(
        "development/servers/volumes/Dockerfile",
        "development/servers/volumes",
        "volumes",
        "latest",
    )?;

    crate::docker::build(
        "development/servers/args/Dockerfile",
        "development/servers/args",
        "args",
        "latest",
    )?;

    // create the internal network
    crate::docker::create_network_if_not_exists("internal")?;

    Ok(())
}

fn check_volumes_work() -> Result<()> {
    // run the main container
    let volumes = vec![
        ("./development", "/development"),
        ("/var/run/docker.sock", "/var/run/docker.sock"),
    ];

    crate::docker::run("f2", "debug", &volumes, "/development/volumes-config.yaml")?;

    // give the container a moment to start up
    std::thread::sleep(Duration::from_secs(1));

    // check we can get a response from it
    let expected = std::fs::read_to_string("development/volumes-configuration.json")?;
    assert_response_equals("http://localhost:3000", &expected)?;

    // remove the running containers
    crate::docker::remove_running_containers()?;

    Ok(())
}

fn check_rolls_work() -> Result<()> {
    // run the main container
    let volumes = vec![
        ("./development", "/development"),
        ("/var/run/docker.sock", "/var/run/docker.sock"),
    ];

    // start the next test
    crate::docker::run(
        "f2",
        "debug",
        &volumes,
        "/development/echo-single-config.yaml",
    )?;

    // give the container a moment to start up
    std::thread::sleep(Duration::from_secs(1));

    // check we can get a response from it
    assert_response_equals("http://localhost:3000/foobar", "Echo foobar")?;

    // roll to a new version
    swap(
        "./development/echo-single-config.yaml",
        "./development/echo-double-config.yaml",
    )?;

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
    assert_response_equals("http://localhost:3000/foobar", "Echo echo foobar")?;

    crate::docker::remove_running_containers()?;

    swap(
        "./development/echo-single-config.yaml",
        "./development/echo-double-config.yaml",
    )?;

    Ok(())
}

fn check_args_work() -> Result<()> {
    let volumes = vec![
        ("./development", "/development"),
        ("/var/run/docker.sock", "/var/run/docker.sock"),
    ];

    // without args: Dockerfile CMD is preserved, server responds with "default"
    crate::docker::run("f2", "debug", &volumes, "/development/args-default-config.yaml")?;
    std::thread::sleep(Duration::from_secs(1));
    assert_response_equals("http://localhost:3000", "default")?;
    crate::docker::remove_running_containers()?;

    // with args: f2 overrides CMD, server responds with "overridden"
    crate::docker::run("f2", "debug", &volumes, "/development/args-override-config.yaml")?;
    std::thread::sleep(Duration::from_secs(1));
    assert_response_equals("http://localhost:3000", "overridden")?;
    crate::docker::remove_running_containers()?;

    Ok(())
}

fn assert_response_equals(uri: &str, content: &str) -> Result<()> {
    let mut response = ureq::get(uri).call()?;
    let response_text = response.body_mut().read_to_string()?;

    if response_text != content {
        return Err(eyre!(
            "Received unexpected response from container: {response_text}, expected: {content} when calling {uri}"
        ));
    }

    println!("✅ Successfully received correct response from {uri}");

    Ok(())
}

fn swap(path1: &str, path2: &str) -> Result<()> {
    std::fs::rename(path1, format!("{}.tmp", path1))?;
    std::fs::rename(path2, path1)?;
    std::fs::rename(format!("{}.tmp", path1), path2)?;

    println!("✅ Successfully swapped {path1} and {path2}");

    Ok(())
}
