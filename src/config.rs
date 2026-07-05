use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub ignore: Vec<String>,
    pub required: Vec<String>,
    pub required_from_examples: bool,
    pub secret_patterns: Vec<String>,
    pub precedence: Vec<String>,
    pub fail_on: FailOn,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Error,
    Warning,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            required: Vec::new(),
            required_from_examples: true,
            secret_patterns: Vec::new(),
            precedence: Vec::new(),
            fail_on: FailOn::Error,
            profiles: BTreeMap::new(),
        }
    }
}
