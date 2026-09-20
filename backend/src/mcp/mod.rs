//! The Model Context Protocol server.
//!
//! Phos already answers every question about a library over REST, so the point
//! of this module is not another API — it is an API shaped for a model rather
//! than for a browser. Three things follow from that:
//!
//! * **Tools are flat and few.** `search_shots` takes the filters a person
//!   would say out loud (a phrase, a person, a date range) rather than mirroring
//!   `GET /api/shots`' query string, and there are six read tools rather than
//!   forty CRUD endpoints. Everything underneath calls the same handler the REST
//!   route calls, so the two can never drift.
//! * **Shots are resources.** A search answers with `resource_link`s to
//!   `phos://shot/{id}`, and the model reads the ones it actually cares about.
//!   Inlining fifty shot records into a tool result would spend the context
//!   window on rows nobody looks at. `resources/list` deliberately returns only
//!   a recent slice: a fifty-thousand-shot library must never be enumerated.
//! * **The model can look.** `get_image` (and `phos://file/{id}`) hands back the
//!   same cached JPEG the web UI shows, as an image content block. A photo
//!   manager whose assistant can only read *about* photos is the wrong shape.
//!
//! Writes are off unless they are asked for — see [`PhosMcp::new`] — and the
//! ones that exist are the reviewer's verbs (rename, merge, assign, confirm),
//! never a delete.
//!
//! Two transports serve the same handler: [`http`] mounts it at `/mcp` for
//! hosted instances, [`stdio`] runs it over a pipe for a local client. [`auth`]
//! is what stands in front of the HTTP one.

pub mod auth;
pub mod http;
pub mod resources;
pub mod stdio;
pub mod tools;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};

use crate::api::AppState;

/// What the model is told the server is for. Clients show this to the model
/// before it picks a tool, so it earns its length by answering the questions a
/// first call gets wrong: what a "shot" is, and which id goes where.
const INSTRUCTIONS: &str = "\
Phos is a self-hosted photo and video library. Its unit is a **shot**: one \
moment, holding one or more **files** (the original plus any edits or \
conversions). Faces detected in those files are grouped into **people**.

Start with `search_shots`. It answers with links to `phos://shot/{id}`; read \
those for the full record, including each file's id. To actually look at a \
photo, call `get_image` with a *file* id (not a shot id) — shot records list \
their files.

Free-text search matches the AI-written caption on each shot and the file path, \
not the contents of the image, so a search that finds nothing does not mean the \
library holds nothing of the kind.";

/// The MCP server: one library, one permission level.
#[derive(Clone)]
pub struct PhosMcp {
    pub(crate) state: AppState,
    router: ToolRouter<PhosMcp>,
}

impl PhosMcp {
    /// Build a server over `state`.
    ///
    /// `writable` decides which tools exist at all, rather than which ones
    /// refuse when called: a tool the model cannot see is a tool it cannot
    /// spend a turn being refused by, and a read-only deployment should not
    /// advertise verbs it will not honour.
    pub fn new(state: AppState, writable: bool) -> Self {
        let router = if writable {
            Self::read_tool_router() + Self::write_tool_router()
        } else {
            Self::read_tool_router()
        };
        Self { state, router }
    }
}

#[rmcp::tool_handler(router = self.router)]
impl ServerHandler for PhosMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "phos",
            env!("PHOS_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        resources::list_recent(&self.state).await
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(resources::templates())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        resources::read(&self.state, &request.uri)
            .await
            .map(Into::into)
    }
}
