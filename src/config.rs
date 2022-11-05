use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct Config {
    pub cell_height: i32,
    pub extra_chars: Vec<String>,
}

impl Config {
    // TODO: split out ConfigFile struct to handle errors properly here.
    pub fn extra_chars<'a>(&'a self) -> impl Iterator<Item = char> + 'a {
        self.extra_chars.iter().filter_map(|s| {
            if let Some(hex) = s.strip_prefix("U+") {
                u32::from_str_radix(hex, 16)
                    .ok()
                    .and_then(|i| i.try_into().ok())
            } else {
                let mut chars = s.chars();
                let result = chars.next();
                if chars.next().is_some() {
                    return None;
                }
                result
            }
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cell_height: 40,
            extra_chars: vec![],
        }
    }
}
