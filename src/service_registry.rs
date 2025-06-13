use std::collections::HashMap;

use indexmap::IndexSet;

use crate::config::Service;
use crate::docker::api::StartedContainerDetails;
use crate::docker::models::ContainerId;

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
    containers: HashMap<String, IndexSet<StartedContainerDetails>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define(&mut self, service: &str, definition: Service) {
        self.definitions.insert(service.to_string(), definition);
    }

    pub fn undefine(&mut self, service: &str) {
        self.definitions.remove(service);
    }

    pub fn get_running_containers(
        &self,
        service: &str,
    ) -> Option<&IndexSet<StartedContainerDetails>> {
        tracing::debug!("Fetching running containers for {service}");

        self.containers.get(service)
    }

    #[tracing::instrument(skip(self))]
    pub fn add_container(&mut self, service: &str, details: StartedContainerDetails) {
        tracing::info!("adding a downstream container");

        self.containers
            .entry(service.to_string())
            .or_default()
            .insert(details);
    }

    pub fn remove_all_containers(&mut self, service: &str) {
        self.containers.remove(service);
    }

    pub fn remove_container_by_id(&mut self, service: &str, id: &ContainerId) {
        if let Some(containers) = self.containers.get_mut(service) {
            containers.retain(|c| c.id != *id);
        }
    }

    pub fn find_downstreams(
        &self,
        host: &str,
        path: &str,
    ) -> Option<(&IndexSet<StartedContainerDetails>, u16)> {
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

    use crate::config::Service;
    use crate::docker::api::StartedContainerDetails;
    use crate::docker::models::ContainerId;
    use crate::service_registry::ServiceRegistry;

    #[test]
    fn can_store_and_fetch_service_definitions() {
        let mut registry = ServiceRegistry::new();
        let service = "backend";

        let definition = Service::default();
        registry.define(service, definition.clone());

        let found = registry.definitions.get(service);

        assert_eq!(found, Some(&definition));
    }

    #[test]
    fn can_remove_service_definitions() {
        let mut registry = ServiceRegistry::new();
        let service = "backend";
        let definition = Service::default();

        registry.define(service, definition);

        assert!(registry.definitions.get(service).is_some());

        registry.undefine(service);

        assert!(registry.definitions.get(service).is_none());
    }

    #[test]
    fn can_store_and_fetch_container_data() {
        let mut registry = ServiceRegistry::new();

        let container1 = ContainerId::random();
        let container2 = ContainerId::random();

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
            host: String::from(host),
            path_prefix,
            ..Default::default()
        };

        registry.define(name, service);
    }

    fn add_container(registry: &mut ServiceRegistry, name: &str) -> ContainerId {
        let id = ContainerId::random();

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

    #[test]
    fn can_remove_container_by_identifier() {
        let mut registry = ServiceRegistry::new();
        let name = "foobar";

        define_service(&mut registry, name, "foo.bar", None);

        let container1 = add_container(&mut registry, name);
        let container2 = add_container(&mut registry, name);

        registry.remove_container_by_id(name, &container1);

        let running_containers = registry
            .get_running_containers(name)
            .expect("Failed to find containers");

        let container1 = running_containers.iter().find(|c| c.id == container1);
        let container2 = running_containers.iter().find(|c| c.id == container2);

        assert!(container1.is_none());
        assert!(container2.is_some());
    }

    #[test]
    fn can_remove_all_containers_for_service() {
        let mut registry = ServiceRegistry::new();
        let name = "foobar";

        define_service(&mut registry, name, "foo.bar", None);

        add_container(&mut registry, name);
        add_container(&mut registry, name);

        registry.remove_all_containers(name);

        assert!(registry.get_running_containers(name).is_none());
    }
}
