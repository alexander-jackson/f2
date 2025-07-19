pub struct PathMatchCalculator<'a> {
    pub path: &'a str,
    pub prefix: Option<&'a str>,
}

impl<'a> PathMatchCalculator<'a> {
    pub fn new(path: &'a str, prefix: Option<&'a str>) -> Self {
        Self { path, prefix }
    }

    pub fn compute_match_length(&self) -> usize {
        let Some(prefix) = self.prefix else {
            return self.path.len();
        };

        self.path.strip_prefix(prefix).map_or(usize::MAX, str::len)
    }
}

#[cfg(test)]
mod tests {
    use crate::service_registry::matching::PathMatchCalculator;

    #[test]
    fn computes_correctly_for_matching_prefix() {
        let path = "/api/v1/resource";
        let prefix = Some("/api/v1");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, 9); // Length of "/resource"
    }

    #[test]
    fn computes_correctly_for_non_matching_prefix() {
        let path = "/api/v1/resource";
        let prefix = Some("/api/v2");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, usize::MAX); // No match, should return usize::MAX
    }

    #[test]
    fn computes_correctly_for_no_prefix() {
        let path = "/api/v1/resource";
        let prefix = None;

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, path.len()); // Should return the length of the path
    }

    #[test]
    fn computes_correctly_for_empty_path() {
        let path = "";
        let prefix = Some("/api/v1");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, usize::MAX); // No match, should return usize::MAX
    }

    #[test]
    fn computes_correctly_for_empty_prefix() {
        let path = "/api/v1/resource";
        let prefix = Some("");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, path.len()); // Should return the length of the path
    }

    #[test]
    fn computes_correctly_for_exact_match() {
        let path = "/api/v1/resource";
        let prefix = Some("/api/v1/resource");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, 0); // Exact match, should return 0
    }

    #[test]
    fn computes_correctly_for_prefix_with_trailing_slash() {
        let path = "/api/v1/resource";
        let prefix = Some("/api/v1/");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, 8); // Length of "/resource"
    }

    #[test]
    fn computes_correctly_for_path_with_trailing_slash() {
        let path = "/api/v1/resource/";
        let prefix = Some("/api/v1");

        let result = PathMatchCalculator::new(path, prefix).compute_match_length();
        assert_eq!(result, 10); // Length of "/resource/"
    }
}
