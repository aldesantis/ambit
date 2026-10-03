//! MCP entity parsing.
//!
//! One shape, one place it can be written: `mcps/<name>.yml` in a catalog. A project that defines a
//! server of its own lists itself as a catalog and puts it there, so this parser has one caller and
//! no variant to reconcile.

use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::expectation::Expectation;
use crate::model::yaml::YamlMapping;
use crate::util::string_enum;

/// A locally-spawned server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioTransport {
    pub command: String,
    pub args: Vec<String>,
    /// Variables the spawned process is given, each name mapped to what supplies it.
    ///
    /// A server reads the names its own author chose, which are not always the names a machine
    /// sets. An entry joins the two, so two servers reading one variable name can be given
    /// different values. `${VAR}` is treated as in [`HttpTransport::headers`], and an `expects`
    /// entry a value references is not also passed under its own name.
    pub env: IndexMap<String, String>,
}

/// A server reached over HTTP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpTransport {
    pub url: String,
    /// The environment variable whose value is sent as the HTTP bearer token.
    pub bearer_token_env_var: Option<String>,
    /// `${VAR}` references are left intact here; at install each harness's profile rewrites them
    /// into the reference syntax that harness expands at spawn time. The value itself is never read.
    pub headers: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpTransport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl McpTransport {
    /// The transport's `kind`, as the TS discriminant spelled it.
    pub fn kind(&self) -> McpTransportKind {
        match self {
            Self::Stdio(_) => McpTransportKind::Stdio,
            Self::Http(_) => McpTransportKind::Http,
        }
    }
}

string_enum! {
    /// The transport kinds ambit understands. `transport` carries exactly one of these as a nested
    /// key, so the kind's own fields stay under it and a new kind adds nothing at the top level.
    pub enum McpTransportKind {
        Http => "http",
        Stdio => "stdio",
    }
}

/// The transport kinds, in the order the TS listed them.
pub const MCP_TRANSPORT_KINDS: &[McpTransportKind] = McpTransportKind::ALL;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpEntity {
    pub name: String,
    pub transport: McpTransport,
    /// What must be true of the world for this server to work: its credentials, today.
    pub expects: Vec<Expectation>,
}

/// Parses one MCP entity: a whole `mcps/*.yml` document.
///
/// # Errors
///
/// Exit 2 for any shape violation.
pub fn parse_mcp_entity(mapping: &YamlMapping) -> Result<McpEntity> {
    let _ = mapping;
    todo!("port model/mcp-entity.ts:parseMcpEntity")
}
