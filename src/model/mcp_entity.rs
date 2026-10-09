use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::expectation::{Expectation, parse_expectations};
use crate::model::yaml::YamlMapping;
use crate::util::cmp::js_cmp;
use crate::util::string_enum;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioTransport {
    pub command: String,
    pub args: Vec<String>,
    pub env: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpTransport {
    pub url: String,
    pub bearer_token_env_var: Option<String>,
    pub headers: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpTransport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl McpTransport {
    pub fn kind(&self) -> McpTransportKind {
        match self {
            Self::Stdio(_) => McpTransportKind::Stdio,
            Self::Http(_) => McpTransportKind::Http,
        }
    }
}

string_enum! {
    pub enum McpTransportKind {
        Http => "http",
        Stdio => "stdio",
    }
}

pub const MCP_TRANSPORT_KINDS: &[McpTransportKind] = McpTransportKind::ALL;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpEntity {
    pub name: String,
    pub transport: McpTransport,
    pub expects: Vec<Expectation>,
}

pub fn parse_mcp_entity(mapping: &YamlMapping) -> Result<McpEntity> {
    mapping.reject_unknown_keys(ENTITY_KEYS)?;

    Ok(McpEntity {
        name: mapping.require_string("name")?,
        transport: parse_transport(mapping)?,
        expects: parse_expectations(mapping)?,
    })
}

const ENTITY_KEYS: &[&str] = &["expects", "name", "transport"];

fn kind_list() -> String {
    MCP_TRANSPORT_KINDS
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_transport(mapping: &YamlMapping) -> Result<McpTransport> {
    let transport = mapping.require_mapping("transport")?;
    let kinds = transport.keys();

    let [kind] = kinds.as_slice() else {
        let message = if kinds.is_empty() {
            "`transport` names no transport kind".to_owned()
        } else {
            let mut sorted = kinds.clone();

            sorted.sort_by(|a, b| js_cmp(a, b));
            format!(
                "`transport` names {} transport kinds: {}",
                kinds.len(),
                sorted.join(", ")
            )
        };

        return Err(mapping.key_error(
            "transport",
            &message,
            vec![
                format!("supported kinds: {}", kind_list()),
                "give `transport` exactly one kind key".to_owned(),
            ],
        ));
    };

    match McpTransportKind::parse(kind) {
        Some(McpTransportKind::Stdio) => {
            let stdio = transport.require_mapping("stdio")?;

            stdio.reject_unknown_keys(&["args", "command", "env"])?;

            Ok(McpTransport::Stdio(StdioTransport {
                command: stdio.require_string("command")?,
                args: stdio.optional_string_list("args")?.unwrap_or_default(),
                env: match stdio.optional_mapping("env")? {
                    Some(env) => env.string_entries()?,
                    None => IndexMap::new(),
                },
            }))
        }
        Some(McpTransportKind::Http) => {
            let http = transport.require_mapping("http")?;

            http.reject_unknown_keys(&["bearer_token_env_var", "headers", "url"])?;

            Ok(McpTransport::Http(HttpTransport {
                url: http.require_string("url")?,
                bearer_token_env_var: http.optional_string("bearer_token_env_var")?,
                headers: match http.optional_mapping("headers")? {
                    Some(headers) => headers.string_entries()?,
                    None => IndexMap::new(),
                },
            }))
        }
        None => Err(transport.key_error(
            kind,
            &format!("unknown transport kind \"{kind}\""),
            vec![
                format!("supported kinds: {}", kind_list()),
                format!("replace `{kind}` with one of them"),
            ],
        )),
    }
}
