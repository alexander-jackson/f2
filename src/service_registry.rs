use std::collections::{HashMap, HashSet};

use crate::config::Service;
use crate::docker::api::StartedContainerDetails;

fn compute_path_prefix_match(path: &str, prefix: Option<&str>) -> usize {
    let Some(prefix) = prefix else {
        return path.len();
    };

    path.strip_prefix(prefix).map_or(usize::MAX, str::len)
}

/// Registry of all of the running services.
#[derive(Debug, Default)]
pub struct ServiceRegistry {
    definitions: HashMap<String, Service>,
    containers: HashMap<String, HashSet<StartedContainerDetails>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_definition(&self, service: &str) -> Option<&Service> {
        self.definitions.get(service)
    }

    pub fn define(&mut self, service: &str, definition: Service) {
        self.definitions.insert(service.to_string(), definition);
    }

    pub fn get_running_containers(
        &self,
        service: &str,
    ) -> Option<&HashSet<StartedContainerDetails>> {
        tracing::debug!("Fetching running containers for {service}");

        self.containers.get(service)
    }

    pub fn add_container(&mut self, service: &str, details: StartedContainerDetails) {
        let StartedContainerDetails { id, addr } = &details;

        tracing::debug!("Adding ({id}, {addr}) as a downstream for {service}");

        self.containers
            .entry(service.to_string())
            .or_default()
            .insert(details);
    }

    pub fn find_downstreams(
        &self,
        host: &str,
        path: &str,
    ) -> Option<(&HashSet<StartedContainerDetails>, u16)> {
        self.definitions
            .iter()
            .filter(|entry| entry.1.host == host)
            .min_by_key(|entry| compute_path_prefix_match(path, entry.1.path_prefix.as_deref()))
            .and_then(|entry| {
                self.get_running_containers(entry.0)
                    .map(|downstreams| (downstreams, entry.1.port))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::Ipv4Addr;

    use rand::rngs::ThreadRng;
    use rand::RngCore;

    use crate::config::Service;
    use crate::docker::api::StartedContainerDetails;
    use crate::docker::models::ContainerId;
    use crate::service_registry::ServiceRegistry;

    fn random_container_id() -> ContainerId {
        let mut rng = ThreadRng::default();
        let mut buf: [u8; 6] = [0; 6];
        rng.fill_bytes(buf.as_mut_slice());

        ContainerId(hex::encode(buf))
    }

    #[test]
    fn can_store_and_fetch_service_definitions() {
        let mut registry = ServiceRegistry::new();
        let service = "backend";

        let definition = Service {
            image: String::from("repo/service"),
            tag: String::from("latest"),
            port: 8080,
            replicas: 1,
            host: String::from("example.com"),
            path_prefix: None,
            environment: None,
        };

        registry.define(service, definition.clone());

        let found = registry.get_definition(service);

        assert_eq!(found, Some(&definition));
    }

    #[test]
    fn can_store_and_fetch_container_data() {
        let mut registry = ServiceRegistry::new();

        let container1 = random_container_id();
        let container2 = random_container_id();

        let first = StartedContainerDetails {
            id: container1.clone(),
            addr: Ipv4Addr::new(127, 0, 0, 3),
        };

        let second = StartedContainerDetails {
            id: container2.clone(),
            addr: Ipv4Addr::new(127, 0, 0, 4),
        };

        registry.add_container("backend", first);
        registry.add_container("backend", second);

        let ids: Option<HashSet<_>> =
            registry
                .get_running_containers("backend")
                .map(|containers| {
                    containers
                        .into_iter()
                        .map(|details| details.id.clone())
                        .collect()
                });

        let mut expected = HashSet::new();
        expected.insert(container1);
        expected.insert(container2);

        assert_eq!(ids, Some(expected));
    }

    fn define_service(
        registry: &mut ServiceRegistry,
        name: &str,
        host: &str,
        path_prefix: Option<String>,
    ) {
        let service = Service {
            image: String::from("something"),
            tag: String::from("latest"),
            port: 80,
            replicas: 1,
            host: String::from(host),
            path_prefix,
            environment: None,
        };

        registry.define(name, service);
    }

    fn add_container(registry: &mut ServiceRegistry, name: &str) -> ContainerId {
        let id = random_container_id();

        let details = StartedContainerDetails {
            id: id.clone(),
            addr: Ipv4Addr::LOCALHOST,
        };

        registry.add_container(name, details);

        id
    }

    fn find_matching_container_ids(
        registry: &ServiceRegistry,
        host: &str,
        path: &str,
    ) -> Option<HashSet<ContainerId>> {
        registry.find_downstreams(host, path).map(|value| {
            value
                .0
                .into_iter()
                .map(|details| details.id.clone())
                .collect()
        })
    }

    #[test]
    fn can_find_downstreams_for_host_and_path_with_one_host_match() {
        let mut registry = ServiceRegistry::new();

        define_service(&mut registry, "opentracker", "opentracker.app", None);
        define_service(&mut registry, "blackboards", "blackboards.pl", None);

        let opentracker_id = add_container(&mut registry, "opentracker");
        add_container(&mut registry, "blackboards");

        let downstreams = find_matching_container_ids(&registry, "opentracker.app", "/foo");

        let mut expected = HashSet::new();
        expected.insert(opentracker_id.clone());

        assert_eq!(downstreams, Some(expected));
    }

    #[test]
    fn can_find_downstreams_for_a_host_and_path_with_multiple_host_matches() {
        let mut registry = ServiceRegistry::new();

        let host = "example.com";

        define_service(&mut registry, "frontend", host, None);
        define_service(&mut registry, "backend", host, Some("/api".into()));

        add_container(&mut registry, "frontend");
        let backend_id = add_container(&mut registry, "backend");

        let downstreams = find_matching_container_ids(&registry, host, "/api/v1/accounts");

        let mut expected = HashSet::new();
        expected.insert(backend_id);

        assert_eq!(downstreams, Some(expected));
    }

    #[test]
    fn produces_no_results_for_downstreams_if_no_matches() {
        let mut registry = ServiceRegistry::new();

        define_service(&mut registry, "frontend", "foo.com", None);
        define_service(&mut registry, "backend", "bar.com", None);

        add_container(&mut registry, "frontend");
        add_container(&mut registry, "backend");

        let downstreams = find_matching_container_ids(&registry, "baz.com", "/boo");

        assert_eq!(downstreams, None);
    }
}
