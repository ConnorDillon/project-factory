use std::collections::HashMap;
use std::fmt::Write;
use std::io::{self, Chain, Cursor, Read};
use std::path::PathBuf;

use log::{debug, info, warn};
use regex::Regex;

use crate::output::TaskId;
use crate::plugin::{Config, FileType, Plugin, PreppedPlugin};

pub struct PreProcessedInput<T> {
    pub task_id: TaskId,
    pub item_path: PathBuf,
    pub item_type: String,
    pub plugin: PreppedPlugin,
    pub data: T,
}

pub struct PreProcessor {
    pub plugins: HashMap<FileType, Plugin>,
    pub compiled: HashMap<FileType, Regex>,
    pub compiled_hex: HashMap<FileType, Regex>,
}

impl PreProcessor {
    pub fn new(config: &Config) -> PreProcessor {
        let compiled = config
            .iter()
            .filter(|(_, s)| !s.header.is_hex())
            .map(|(t, s)| (t.clone(), Regex::new(&s.header.regex).unwrap()))
            .collect();
        let compiled_hex = config
            .iter()
            .filter(|(_, s)| s.header.is_hex())
            .map(|(t, s)| {
                let mut re = s.header.regex.replace(" ", "");
                re.make_ascii_uppercase();
                (t.clone(), Regex::new(&re).unwrap())
            })
            .collect();
        PreProcessor {
            plugins: config
                .iter()
                .map(|(t, s)| (t.clone(), s.plugin.clone()))
                .collect(),
            compiled,
            compiled_hex,
        }
    }

    pub fn pre_process<R: Read>(
        &self,
        task_id: TaskId,
        item_path: PathBuf,
        file_path: Option<&PathBuf>,
        mut data: R,
    ) -> io::Result<Option<PreProcessedInput<Chain<Cursor<Vec<u8>>, R>>>> {
        let mut buf = Vec::with_capacity(4096);
        (&mut data).take(4096).read_to_end(&mut buf)?;
        match self.get_file_type(&buf) {
            Some(item_type) => match self.plugins.get(&item_type) {
                Some(plugin) => {
                    let pplugin = plugin.prep(file_path)?;
                    debug!("{}: Prepped plugin: {:?}", task_id, pplugin);
                    info!(
                        "{}: Processing {:?} type: {} with plugin: {}",
                        task_id, item_path, item_type, pplugin.plugin_name
                    );
                    Ok(Some(PreProcessedInput {
                        task_id,
                        item_path,
                        item_type,
                        plugin: pplugin,
                        data: Cursor::new(buf).chain(data),
                    }))
                }
                None => {
                    warn!(
                        "{}: File type for {:?} not included in config: {}",
                        task_id, item_path, item_type
                    );
                    Ok(None)
                }
            },
            None => {
                warn!(
                    "{}: File type for {:?} was not determined",
                    task_id, item_path
                );
                Ok(None)
            }
        }
    }

    fn get_file_type(&self, head: &[u8]) -> Option<FileType> {
        let head_str = String::from_utf8_lossy(head);
        for (t, r) in self.compiled.iter() {
            if r.is_match(&head_str) {
                return Some(t.clone());
            }
        }
        let mut head_hex = String::with_capacity(head.len() * 2);
        for byte in head {
            write!(head_hex, "{:02X}", byte).unwrap();
        }
        for (t, r) in self.compiled_hex.iter() {
            if r.is_match(&head_hex) {
                return Some(t.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plugin::{Header, Plugin, Settings};

    fn empty_plugin() -> Plugin {
        Plugin {
            name: "".into(),
            path: "".into(),
            args: None,
            input: None,
            output: None,
            unpacker: None,
        }
    }

    #[test]
    fn test_get_file_type() {
        let conf = vec![(
            "foo".into(),
            Settings {
                header: Header {
                    regex: "^.FOO".into(),
                    hex: None,
                },
                plugin: empty_plugin(),
            },
        )]
        .into_iter()
        .collect();
        let pp = PreProcessor::new(&conf);
        assert_eq!(
            pp.get_file_type(&[0x8b, 0x46, 0x4f, 0x4f, 0x8b]),
            Some("foo".into())
        );
    }

    #[test]
    fn test_get_file_type_hex() {
        let conf = vec![(
            "bar".into(),
            Settings {
                header: Header {
                    regex: "^8B 00 .. 4f4F$".into(),
                    hex: Some(true),
                },
                plugin: empty_plugin(),
            },
        )]
        .into_iter()
        .collect();
        let pp = PreProcessor::new(&conf);
        assert_eq!(
            pp.get_file_type(&[0x8b, 0x00, 0x46, 0x4f, 0x4f]),
            Some("bar".into())
        );
    }
}
