use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{AppState, routes};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "clipos API",
        description = "Invite-only clip archive.",
        license(name = "MIT", identifier = "MIT")
    ),
    modifiers(&BearerAuth),
    components(schemas(crate::ErrorBody))
)]
struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
    }
}

/// Routes mounted under `/api`, together with their OpenAPI description. Paths in the
/// spec are prefixed with `/api` to match how the SPA calls them.
pub fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::config::config))
        .routes(routes!(routes::me::me, routes::me::update_me))
        .routes(routes!(
            routes::admin::list_invites,
            routes::admin::create_invite
        ))
        .routes(routes!(routes::admin::revoke_invite))
        .routes(routes!(routes::admin::list_users))
        .routes(routes!(routes::admin::update_user))
        .routes(routes!(routes::admin::analyse_clip))
        .routes(routes!(routes::admin::list_duplicates))
        .routes(routes!(
            routes::clips::list_clips,
            routes::clips::create_clip
        ))
        .routes(routes!(
            routes::clips::get_clip,
            routes::clips::update_clip,
            routes::clips::delete_clip
        ))
        .routes(routes!(routes::clips::check_upload))
        .routes(routes!(routes::clips::complete_clip))
        .routes(routes!(routes::clips::retry_clip))
        .routes(routes!(routes::clips::hold_clip))
        .routes(routes!(routes::clips::release_clip))
        .routes(routes!(routes::clips::restore_clip))
        .routes(routes!(routes::clips::download_clip))
        .routes(routes!(routes::clips::get_analysis))
        .routes(routes!(
            routes::clips::add_reaction,
            routes::clips::remove_reaction
        ))
        .routes(routes!(routes::clips::trash))
        .routes(routes!(
            routes::share::share_clip,
            routes::share::unshare_clip
        ))
        .routes(routes!(routes::members::list_members))
        .routes(routes!(routes::members::get_profile))
        .routes(routes!(routes::members::search_tags))
        .routes(routes!(routes::shows::tonight))
        .routes(routes!(routes::shows::current_show))
        .routes(routes!(
            routes::shows::list_shows,
            routes::shows::create_show
        ))
        .routes(routes!(routes::shows::get_show))
        .routes(routes!(routes::shows::set_lineup))
        .routes(routes!(routes::shows::add_clip))
        .routes(routes!(routes::shows::join_show))
        .routes(routes!(routes::shows::set_ready))
        .routes(routes!(routes::shows::start_show))
        .routes(routes!(routes::shows::mark_played))
        .routes(routes!(routes::shows::react))
        .routes(routes!(routes::shows::finale))
        .routes(routes!(routes::shows::vote))
        .routes(routes!(routes::shows::end_show))
}

/// The OpenAPI document as served under `/api`, used to generate the web client types.
/// Nesting keeps the outer document's `info`, so the outer one is `ApiDoc` too.
pub fn spec() -> utoipa::openapi::OpenApi {
    let mut spec = OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        .nest("/api", api_router())
        .into_openapi();
    unauthorized_on_signed_in_routes(&mut spec);
    spec
}

/// Every route that needs a token also answers 401 without a valid one (`extract`).
fn unauthorized_on_signed_in_routes(spec: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::{ContentBuilder, Ref, ResponseBuilder};
    let unauthorized = ResponseBuilder::new()
        .description("No token, or an invalid or expired one")
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(Ref::from_schema_name("ErrorBody")))
                .build(),
        )
        .build();
    for item in spec.paths.paths.values_mut() {
        let operations = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.patch,
        ];
        for op in operations.into_iter().flatten() {
            if op.security.as_ref().is_some_and(|s| !s.is_empty()) {
                op.responses
                    .responses
                    .entry("401".into())
                    .or_insert_with(|| unauthorized.clone().into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// `make gen-api` writes `web/openapi.json` from `spec()`, and the web client's types
    /// come from that file: it must match the routes as built.
    #[test]
    fn the_committed_spec_is_current() {
        let committed = include_str!("../../../web/openapi.json");
        let built = format!("{}\n", spec().to_pretty_json().unwrap());
        assert!(
            built == committed,
            "web/openapi.json is stale; run `make gen-api` and commit it"
        );
    }

    /// The document is clipos's own, not utoipa-axum's (which an outer router built with
    /// `OpenApiRouter::new()` would bring: its name, maintainer and licence).
    #[test]
    fn the_spec_describes_clipos() {
        let info = spec().info;
        assert_eq!(info.title, "clipos API");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.license.map(|l| l.name).as_deref(), Some("MIT"));
        assert!(info.contact.is_none());
    }

    #[test]
    fn signed_in_routes_document_their_401() {
        let spec = serde_json::to_value(spec()).unwrap();
        let mut secured = 0;
        for (path, item) in spec["paths"].as_object().unwrap() {
            assert!(path.starts_with("/api/"), "{path}");
            for (method, op) in item.as_object().unwrap() {
                let signed_in = op
                    .get("security")
                    .and_then(Value::as_array)
                    .is_some_and(|s| !s.is_empty());
                let unauthorized = &op["responses"]["401"];
                if signed_in {
                    secured += 1;
                    assert_eq!(
                        unauthorized["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/ErrorBody",
                        "{method} {path}"
                    );
                } else {
                    assert!(unauthorized.is_null(), "{method} {path}");
                }
            }
        }
        assert!(secured > 30, "{secured}");
        // The one public route.
        assert!(spec["paths"]["/api/config"]["get"]["security"].is_null());
        assert_eq!(
            spec["components"]["securitySchemes"]["bearer"],
            serde_json::json!({ "type": "http", "scheme": "bearer", "bearerFormat": "JWT" })
        );
    }
}
