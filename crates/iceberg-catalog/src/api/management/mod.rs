#![allow(deprecated)]

pub mod v1 {
    pub mod bootstrap;
    pub mod namespace;
    pub mod project;
    pub mod role;
    pub mod table;
    pub mod user;
    pub mod view;
    pub mod warehouse;

    use std::marker::PhantomData;

    use axum::{
        extract::{Path, Query, State as AxumState},
        response::{IntoResponse, Response},
        routing::{get, post},
        Extension, Json, Router,
    };
    use bootstrap::{BootstrapRequest, ServerInfo, Service as _};
    use http::StatusCode;
    use iceberg_ext::catalog::rest::ErrorModel;
    use namespace::NamespaceManagementService as _;
    use project::{
        CreateProjectRequest, CreateProjectResponse, GetProjectResponse, ListProjectsResponse,
        RenameProjectRequest, Service as _,
    };
    use role::{
        CreateRoleRequest, ListRolesQuery, ListRolesResponse, Role, SearchRoleRequest,
        SearchRoleResponse, Service as _, UpdateRoleRequest,
    };
    use serde::{Deserialize, Serialize};
    use table::TableManagementService as _;
    use user::{
        CreateUserRequest, SearchUserRequest, SearchUserResponse, Service as _, UpdateUserRequest,
        User,
    };
    use utoipa::{openapi::security::SecurityScheme, OpenApi, ToSchema};
    use view::ViewManagementService as _;
    use warehouse::{
        CreateWarehouseRequest, CreateWarehouseResponse, GetWarehouseResponse,
        ListDeletedTabularsQuery, ListWarehousesRequest, ListWarehousesResponse,
        RenameWarehouseRequest, Service as _, UpdateWarehouseCredentialRequest,
        UpdateWarehouseDeleteProfileRequest, UpdateWarehouseStorageRequest,
        WarehouseStatisticsResponse,
    };

    use crate::{
        api::{
            endpoints::ManagementV1Endpoint,
            iceberg::{types::PageToken, v1::PaginationQuery},
            management::v1::{
                project::{EndpointStatisticsResponse, GetEndpointStatisticsRequest},
                user::{ListUsersQuery, ListUsersResponse},
                warehouse::UndropTabularsRequest,
            },
            ApiContext, IcebergErrorResponse, Result,
        },
        request_metadata::RequestMetadata,
        service::{
            authn::UserId, authz::Authorizer, Actor, Catalog, CreateOrUpdateUserResponse,
            NamespaceIdentUuid, RoleId, SecretStore, State, TableIdentUuid, TabularIdentUuid,
            ViewIdentUuid,
        },
        ProjectId, WarehouseIdent,
    };

    pub(crate) fn default_page_size() -> i64 {
        100
    }

    #[derive(Debug, OpenApi)]
    #[openapi(
        info(
            title = "Lakekeeper Management API",
            description = "Lakekeeper is a rust-native Apache Iceberg REST Catalog implementation. The Management API provides endpoints to manage the server, projects, warehouses, users, and roles. If Authorization is enabled, permissions can also be managed. An interactive Swagger-UI for the specific Lakekeeper Version and configuration running is available at `/swagger-ui/#/` of Lakekeeper (by default [http://localhost:8181/swagger-ui/#/](http://localhost:8181/swagger-ui/#/)).",
        ),
        tags(
            (name = "server", description = "Manage Server"),
            (name = "project", description = "Manage Projects"),
            (name = "warehouse", description = "Manage Warehouses"),
            (name = "user", description = "Manage Users"),
            (name = "role", description = "Manage Roles")
        ),
        security(
            ("bearerAuth" = [])
        ),
        paths(
            activate_warehouse,
            bootstrap,
            create_project,
            create_role,
            create_user,
            create_warehouse,
            deactivate_warehouse,
            delete_default_project,
            delete_default_project_deprecated,
            delete_project_by_id,
            delete_role,
            delete_user,
            delete_warehouse,
            get_default_project,
            get_default_project_deprecated,
            get_endpoint_statistics,
            get_project_by_id,
            get_role,
            get_server_info,
            get_user,
            get_warehouse,
            get_warehouse_statistics,
            list_deleted_tabulars,
            list_projects,
            list_roles,
            list_user,
            list_warehouses,
            rename_default_project,
            rename_default_project_deprecated,
            rename_project_by_id,
            rename_warehouse,
            search_role,
            search_user,
            set_namespace_protection,
            set_table_protection,
            set_view_protection,
            set_warehouse_protection,
            get_namespace_protection,
            get_table_protection,
            get_view_protection,
            undrop_tabulars,
            undrop_tabulars_deprecated,
            update_role,
            update_storage_credential,
            update_storage_profile,
            update_user,
            update_warehouse_delete_profile,
            whoami,
        ),
        modifiers(&SecurityAddon)
    )]
    struct ManagementApiDoc;

    struct SecurityAddon;

    impl utoipa::Modify for SecurityAddon {
        fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
            let components = openapi.components.as_mut().unwrap(); // we can unwrap safely since there already is components registered.
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }

    #[derive(Clone, Debug)]
    pub struct ApiServer<C: Catalog, A: Authorizer + Clone, S: SecretStore> {
        auth_handler: PhantomData<A>,
        config_server: PhantomData<C>,
        secret_store: PhantomData<S>,
    }

    /// Get information about the server
    #[utoipa::path(
        get,
        tag = "server",
        path = ManagementV1Endpoint::ServerInfo.path(),
        responses(
            (status = 200, description = "Server info", body = ServerInfo),
            (status = "4XX", body = IcebergErrorResponse),
            (status = 500, description = "Unauthorized", body = IcebergErrorResponse)
        )
    )]
    async fn get_server_info<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, Json<ServerInfo>)> {
        ApiServer::<C, A, S>::server_info(api_context, metadata)
            .await
            .map(|user| (StatusCode::OK, Json(user)))
    }

    /// Creates the user in the catalog if it does not exist.
    /// If the user exists, it updates the users' metadata from the token.
    /// The token sent to this endpoint should have "profile" and "email" scopes.
    #[utoipa::path(
        post,
        tag = "server",
        path = ManagementV1Endpoint::Bootstrap.path(),
        request_body = BootstrapRequest,
        responses(
            (status = 204, description = "Server bootstrapped successfully"),
            (status = "4XX", body = IcebergErrorResponse),
            (status = 500, description = "InternalError", body = IcebergErrorResponse)
        )
    )]
    async fn bootstrap<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<BootstrapRequest>,
    ) -> Result<StatusCode> {
        ApiServer::<C, A, S>::bootstrap(api_context, metadata, request).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Creates the user in the catalog if it does not exist.
    /// If the user exists, it updates the users' metadata from the token.
    /// The token sent to this endpoint should have "profile" and "email" scopes.
    #[utoipa::path(
        post,
        tag = "user",
        path = ManagementV1Endpoint::CreateUser.path(),
        request_body = CreateUserRequest,
        responses(
            (status = 200, description = "User updated", body = User),
            (status = 201, description = "User created", body = User),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn create_user<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<CreateUserRequest>,
    ) -> Result<(StatusCode, Json<User>)> {
        ApiServer::<C, A, S>::create_user(api_context, metadata, request)
            .await
            .map(|u| match u {
                CreateOrUpdateUserResponse::Created(user) => (StatusCode::CREATED, Json(user)),
                CreateOrUpdateUserResponse::Updated(user) => (StatusCode::OK, Json(user)),
            })
    }

    /// Search for users (Fuzzy)
    #[utoipa::path(
        post,
        tag = "user",
        path = ManagementV1Endpoint::SearchUser.path(),
        request_body = SearchUserRequest,
        responses(
            (status = 200, description = "List of users", body = SearchUserResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn search_user<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<SearchUserRequest>,
    ) -> Result<SearchUserResponse> {
        ApiServer::<C, A, S>::search_user(api_context, metadata, request).await
    }

    /// Get a user by ID
    #[utoipa::path(
        get,
        tag = "user",
        path = ManagementV1Endpoint::GetUser.path(),
        params(("user_id" = String,)),
        responses(
            (status = 200, description = "User details", body = User),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_user<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(user_id): Path<UserId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, Json<User>)> {
        ApiServer::<C, A, S>::get_user(api_context, metadata, user_id)
            .await
            .map(|user| (StatusCode::OK, Json(user)))
    }

    /// Get the currently authenticated user
    #[utoipa::path(
        get,
        tag = "user",
        path = ManagementV1Endpoint::Whoami.path(),
        responses(
            (status = 200, description = "User details", body = User),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn whoami<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, Json<User>)> {
        let id = match metadata.actor() {
            Actor::Role { principal, .. } | Actor::Principal(principal) => principal.clone(),
            Actor::Anonymous => {
                return Err(ErrorModel::unauthorized(
                    "No token provided",
                    "GetMyUserWithoutToken",
                    None,
                )
                .into())
            }
        };

        ApiServer::<C, A, S>::get_user(api_context, metadata, id)
            .await
            .map(|user| (StatusCode::OK, Json(user)))
    }

    /// Update details of a user. Replaces the current details with the new details.
    /// If a field is not provided, it is set to `None`.
    #[utoipa::path(
        put,
        tag = "user",
        path = ManagementV1Endpoint::UpdateUser.path(),
        params(("user_id" = String,)),
        request_body = UpdateUserRequest,
        responses(
            (status = 200, description = "User details updated successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn update_user<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(user_id): Path<UserId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UpdateUserRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::update_user(api_context, metadata, user_id, request).await
    }

    /// List users
    #[utoipa::path(
        get,
        tag = "user",
        path = ManagementV1Endpoint::ListUser.path(),
        params(ListUsersQuery),
        responses(
            (status = 200, description = "List of users", body = ListUsersResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn list_user<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Query(query): Query<ListUsersQuery>,
    ) -> Result<ListUsersResponse> {
        ApiServer::<C, A, S>::list_user(api_context, metadata, query).await
    }

    /// Delete user
    ///
    /// All permissions of the user are permanently removed and need to be re-added
    /// if the user is re-registered.
    #[utoipa::path(
        delete,
        tag = "user",
        path =  ManagementV1Endpoint::DeleteUser.path(),
        params(("user_id" = String,)),
        responses(
            (status = 204, description = "User deleted successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn delete_user<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(user_id): Path<UserId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_user(api_context, metadata, user_id)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Create a new role
    #[utoipa::path(
        post,
        tag = "role",
        path = ManagementV1Endpoint::CreateRole.path(),
        request_body = CreateRoleRequest,
        responses(
            (status = 201, description = "Role successfully created", body = Role),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn create_role<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<CreateRoleRequest>,
    ) -> Response {
        match ApiServer::<C, A, S>::create_role(request, api_context, metadata).await {
            Ok(role) => (StatusCode::CREATED, Json(role)).into_response(),
            Err(e) => e.into_response(),
        }
    }

    /// Search for roles (Fuzzy)
    #[utoipa::path(
        post,
        tag = "role",
        path = ManagementV1Endpoint::SearchRole.path(),
        request_body = SearchRoleRequest,
        responses(
            (status = 200, description = "List of users", body = SearchRoleResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn search_role<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<SearchRoleRequest>,
    ) -> Result<SearchRoleResponse> {
        ApiServer::<C, A, S>::search_role(api_context, metadata, request).await
    }

    /// List roles in a project
    #[utoipa::path(
        get,
        tag = "role",
        path = ManagementV1Endpoint::ListRole.path(),
        params(ListRolesQuery),
        responses(
            (status = 200, description = "List of roles", body = ListRolesResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn list_roles<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Query(query): Query<ListRolesQuery>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<ListRolesResponse> {
        ApiServer::<C, A, S>::list_roles(api_context, query, metadata).await
    }

    /// Delete role
    ///
    /// All permissions of the role are permanently removed.
    #[utoipa::path(
        delete,
        tag = "role",
        path = ManagementV1Endpoint::DeleteRole.path(),
        params(("role_id" = Uuid,)),
        responses(
            (status = 204, description = "Role deleted successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn delete_role<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(role_id): Path<RoleId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_role(api_context, metadata, role_id)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Get a role
    #[utoipa::path(
        get,
        tag = "role",
        path = ManagementV1Endpoint::GetRole.path(),
        params(("role_id" = Uuid,)),
        responses(
            (status = 200, description = "Role details", body = Role),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_role<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(role_id): Path<RoleId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, Json<Role>)> {
        ApiServer::<C, A, S>::get_role(api_context, metadata, role_id)
            .await
            .map(|role| (StatusCode::OK, Json(role)))
    }

    /// Update a role
    #[utoipa::path(
        post,
        tag = "role",
        path = ManagementV1Endpoint::UpdateRole.path(),
        params(("role_id" = Uuid,)),
        request_body = UpdateRoleRequest,
        responses(
            (status = 200, description = "Role updated successfully", body = Role),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn update_role<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(role_id): Path<RoleId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UpdateRoleRequest>,
    ) -> Result<(StatusCode, Json<Role>)> {
        ApiServer::<C, A, S>::update_role(api_context, metadata, role_id, request)
            .await
            .map(|role| (StatusCode::OK, Json(role)))
    }

    /// Create a new warehouse.
    ///
    /// Create a new warehouse in the given project. The project
    /// of a warehouse cannot be changed after creation.
    /// The storage configuration is validated by this method.
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::CreateWarehouse.path(),
        request_body = CreateWarehouseRequest,
        responses(
            (status = 201, description = "Warehouse created successfully", body = CreateWarehouseResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn create_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<CreateWarehouseRequest>,
    ) -> Result<CreateWarehouseResponse> {
        ApiServer::<C, A, S>::create_warehouse(request, api_context, metadata).await
    }

    /// List all projects the requesting user has access to
    #[utoipa::path(
        get,
        tag = "project",
        path = ManagementV1Endpoint::ListProjects.path(),
        responses(
            (status = 200, description = "List of projects", body = ListProjectsResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn list_projects<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<ListProjectsResponse> {
        ApiServer::<C, A, S>::list_projects(api_context, metadata).await
    }

    /// Create a new project
    #[utoipa::path(
        post,
        tag = "project",
        path = ManagementV1Endpoint::CreateProject.path(),
        responses(
            (status = 201, description = "Project created successfully", body = CreateProjectResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn create_project<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<CreateProjectRequest>,
    ) -> Result<CreateProjectResponse> {
        ApiServer::<C, A, S>::create_project(request, api_context, metadata).await
    }

    /// Get the default project
    #[utoipa::path(
        get,
        tag = "project",
        path = ManagementV1Endpoint::GetDefaultProject.path(),
        responses(
            (status = 200, description = "Project details", body = GetProjectResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_default_project<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<GetProjectResponse> {
        ApiServer::<C, A, S>::get_project(None, api_context, metadata).await
    }

    /// Get the default project. This Endpoint is deprecated.
    #[utoipa::path(
            get,
            tag = "project",
            path = ManagementV1Endpoint::GetDefaultProjectDeprecated.path(),
            responses(
                (status = 200, description = "Project details", body = GetProjectResponse),
                (status = "4XX", body = IcebergErrorResponse),
            )
        )]
    #[deprecated(
        since = "0.8.0",
        note = "This endpoint is deprecated and will be removed in a future version. Use `/v1/projects/default` instead."
    )]
    async fn get_default_project_deprecated<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<GetProjectResponse> {
        ApiServer::<C, A, S>::get_project(None, api_context, metadata).await
    }

    /// Get a specific project by id
    #[utoipa::path(
        get,
        tag = "project",
        path = ManagementV1Endpoint::GetDefaultProjectById.path(),
        params(("project_id" = String,)),
        responses(
            (status = 200, description = "Project details", body = GetProjectResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_project_by_id<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(project_id): Path<ProjectId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<GetProjectResponse> {
        ApiServer::<C, A, S>::get_project(Some(project_id), api_context, metadata).await
    }

    /// Delete the default project
    #[utoipa::path(
        delete,
        tag = "project",
        path = ManagementV1Endpoint::DeleteDefaultProject.path(),
        responses(
            (status = 204, description = "Project deleted successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn delete_default_project<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_project(None, api_context, metadata)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Delete the default project
    #[utoipa::path(
            delete,
            tag = "project",
            path = ManagementV1Endpoint::DeleteDefaultProjectDeprecated .path(),
            responses(
                (status = 204, description = "Project deleted successfully"),
                (status = "4XX", body = IcebergErrorResponse),
            )
        )]
    #[deprecated(
        since = "0.8.0",
        note = "This endpoint is deprecated and will be removed in a future version. Use `/v1/projects/default` instead."
    )]
    async fn delete_default_project_deprecated<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_project(None, api_context, metadata)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Delete a project by ID
    #[utoipa::path(
        delete,
        tag = "project",
        path = ManagementV1Endpoint::DeleteProjectById.path(),
        params(("project_id" = String,)),
        responses(
            (status = 204, description = "Project deleted successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn delete_project_by_id<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(project_id): Path<ProjectId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_project(Some(project_id), api_context, metadata)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Rename the default project.
    #[utoipa::path(
        post,
        tag = "project",
        path = ManagementV1Endpoint::RenameDefaultProject.path(),
        responses(
            (status = 200, description = "Project renamed successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn rename_default_project<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<RenameProjectRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::rename_project(None, request, api_context, metadata).await
    }

    /// Rename the default project.
    /// This Endpoint is deprecated, use `/v1/projects/default` instead.
    #[utoipa::path(
            post,
            tag = "project",
            path = ManagementV1Endpoint::RenameDefaultProjectDeprecated.path(),
            responses(
                (status = 200, description = "Project renamed successfully"),
                (status = "4XX", body = IcebergErrorResponse),
            )
        )]
    #[deprecated(
        since = "0.8.0",
        note = "This endpoint is deprecated and will be removed in a future version. Use `/v1/projects/default` instead."
    )]
    async fn rename_default_project_deprecated<C: Catalog, A: Authorizer, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<RenameProjectRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::rename_project(None, request, api_context, metadata).await
    }

    /// Rename project by id
    #[utoipa::path(
        post,
        tag = "project",
        path = ManagementV1Endpoint::RenameProjectById.path(),
        params(("project_id" = String,)),
        responses(
            (status = 200, description = "Project renamed successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn rename_project_by_id<C: Catalog, A: Authorizer, S: SecretStore>(
        Path(project_id): Path<ProjectId>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<RenameProjectRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::rename_project(Some(project_id), request, api_context, metadata).await
    }

    /// List all warehouses in a project
    ///
    /// By default, this endpoint does not return deactivated warehouses.
    /// To include deactivated warehouses, set the `include_deactivated` query parameter to `true`.
    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::ListWarehouses.path(),
        params(ListWarehousesRequest),
        responses(
            (status = 200, description = "List of warehouses", body = ListWarehousesResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn list_warehouses<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Query(request): Query<ListWarehousesRequest>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<ListWarehousesResponse> {
        ApiServer::<C, A, S>::list_warehouses(request, api_context, metadata).await
    }

    /// Get a warehouse by ID
    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::GetWarehouse.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 200, description = "Warehouse details", body = GetWarehouseResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<GetWarehouseResponse> {
        ApiServer::<C, A, S>::get_warehouse(warehouse_id.into(), api_context, metadata).await
    }

    #[derive(Debug, Deserialize, utoipa::IntoParams)]
    pub struct DeleteWarehouseQuery {
        #[serde(
            deserialize_with = "crate::api::iceberg::types::deserialize_bool",
            default
        )]
        pub(crate) force: bool,
    }

    /// Delete a warehouse by ID
    #[utoipa::path(
        delete,
        tag = "warehouse",
        path = ManagementV1Endpoint::DeleteWarehouse.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 204, description = "Warehouse deleted successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn delete_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        Query(query): Query<DeleteWarehouseQuery>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<(StatusCode, ())> {
        ApiServer::<C, A, S>::delete_warehouse(warehouse_id.into(), query, api_context, metadata)
            .await
            .map(|()| (StatusCode::NO_CONTENT, ()))
    }

    /// Rename a warehouse
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::RenameWarehouse.path(),
        params(("warehouse_id" = Uuid,)),
        request_body = RenameWarehouseRequest,
        responses(
            (status = 200, description = "Warehouse renamed successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn rename_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<RenameWarehouseRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::rename_warehouse(warehouse_id.into(), request, api_context, metadata)
            .await
    }

    /// Update the Deletion Profile (soft-delete) of a warehouse.
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::UpdateWarehouseDeleteProfile.path(),
        params(("warehouse_id" = Uuid,)),
        request_body = UpdateWarehouseDeleteProfileRequest,
        responses(
            (status = 200, description = "Deletion Profile updated successfully"),
        (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn update_warehouse_delete_profile<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UpdateWarehouseDeleteProfileRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::update_warehouse_delete_profile(
            warehouse_id.into(),
            request,
            api_context,
            metadata,
        )
        .await
    }

    /// Deactivate a warehouse
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::DeactivateWarehouse.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 200, description = "Warehouse deactivated successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn deactivate_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::deactivate_warehouse(warehouse_id.into(), api_context, metadata).await
    }

    /// Activate a warehouse
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::ActivateWarehouse.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 200, description = "Warehouse activated successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn activate_warehouse<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::activate_warehouse(warehouse_id.into(), api_context, metadata).await
    }

    /// Update the storage profile of a warehouse including its storage credential.
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::UpdateStorageProfile.path(),
        params(("warehouse_id" = Uuid,)),
        request_body = UpdateWarehouseStorageRequest,
        responses(
            (status = 200, description = "Storage profile updated successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn update_storage_profile<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UpdateWarehouseStorageRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::update_storage(warehouse_id.into(), request, api_context, metadata)
            .await
    }

    /// Update the storage credential of a warehouse. The storage profile is not modified.
    /// This can be used to update credentials before expiration.
    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::UpdateStorageCredential.path(),
        params(("warehouse_id" = Uuid,)),
        request_body = UpdateWarehouseCredentialRequest,
        responses(
            (status = 200, description = "Storage credential updated successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn update_storage_credential<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UpdateWarehouseCredentialRequest>,
    ) -> Result<()> {
        ApiServer::<C, A, S>::update_storage_credential(
            warehouse_id.into(),
            request,
            api_context,
            metadata,
        )
        .await
    }

    #[derive(Serialize, Deserialize)]
    struct RecursiveDeleteQuery {
        #[serde(default)]
        force: bool,
        #[serde(default)]
        purge: bool,
    }

    #[derive(Deserialize, Debug, ToSchema)]
    pub struct SetProtectionRequest {
        /// Setting this to `true` will prevent the entity from being deleted unless `force` is used.
        pub protected: bool,
    }

    #[derive(Debug, Deserialize, Serialize, utoipa::IntoParams)]
    pub struct GetWarehouseStatisticsQuery {
        /// Next page token
        #[serde(skip_serializing_if = "PageToken::skip_serialize")]
        #[param(value_type=String)]
        pub page_token: PageToken,
        /// Signals an upper bound of the number of results that a client will receive.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(default)]
        pub page_size: Option<i64>,
    }

    impl GetWarehouseStatisticsQuery {
        fn to_pagination_query(&self) -> PaginationQuery {
            PaginationQuery {
                page_token: self.page_token.clone(),
                page_size: self.page_size,
            }
        }
    }

    /// Get warehouse statistics
    ///
    /// Get statistics about the warehouse, currently table and view counts over time.
    ///
    /// We lazily create a new statistics entry every hour, in between hours, the existing entry is
    /// being updated. If there's a change at created_at + 1 hour, a new entry is created.
    /// If there's been no change, no new entry is created, meaning there may be gaps.
    ///
    /// Example:
    /// - 00:16:32: warehouse created:
    ///     - timestamp: 01:00:00, created_at: 00:16:32, updated_at: null, 0 tables, 0 views
    /// - 00:30:00: table created:
    ///     - timestamp: 01:00:00, created_at: 00:16:32, updated_at: 00:30:00, 1 table, 0 views
    /// - 00:45:00: view created:
    ///     - timestamp: 01:00:00, created_at: 00:16:32, updated_at: 00:45:00, 1 table, 1 view
    /// - 01:00:36: table deleted:
    ///     - timestamp: 02:00:00, created_at: 01:00:36, updated_at: null, 0 tables, 1 view
    ///     - timestamp: 01:00:00, created_at: 00:16:32, updated_at: 00:45:00, 1 table, 1 view
    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::GetWarehouseStatistics.path(),
        params(("warehouse_id" = Uuid,), GetWarehouseStatisticsQuery),
        responses(
            (status = 200, description = "Warehouse statistics", body = WarehouseStatisticsResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_warehouse_statistics<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        Query(query): Query<GetWarehouseStatisticsQuery>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<Json<WarehouseStatisticsResponse>> {
        ApiServer::<C, A, S>::get_warehouse_statistics(
            warehouse_id.into(),
            query,
            api_context,
            metadata,
        )
        .await
        .map(Json)
    }

    /// Retrieve API Usage Statistics
    ///
    /// Returns detailed endpoint call statistics for your project, allowing you to monitor API usage patterns,
    /// track frequency of operations, and analyze response codes.
    ///
    /// ## Data Collection
    ///
    /// The statistics include:
    /// - Endpoint paths and HTTP methods
    /// - Response status codes
    /// - Call counts per endpoint
    /// - Warehouse context (when applicable)
    /// - Timestamps of activity
    ///
    /// ## Time Aggregation
    ///
    /// Statistics are aggregated hourly. Within each hour window:
    /// - An initial entry is created on the first API call
    /// - Subsequent calls update the existing hourly entry
    /// - Each hour boundary creates a new aggregation bucket
    /// - Hours with no API activity have no entries (gaps in data)
    ///
    /// ## Response Format
    ///
    /// The response includes timestamp buckets (in UTC) and corresponding endpoint metrics,
    /// allowing for time-series analysis of API usage patterns.
    ///
    /// Example:
    /// - 00:00:00-00:16:32: no activity
    ///     - timestamps: []
    /// - 00:16:32: warehouse created:
    ///     - timestamps: ["01:00:00"], called_endpoints: [[{"count": 1, "http_route": "POST /management/v1/warehouse", "status_code": 201, "warehouse_id": null, "warehouse_name": null, "created_at": "00:16:32", "updated_at": null}]]
    /// - 00:30:00: table created:
    ///     - timestamps: ["01:00:00"], called_endpoints: [[{"count": 1, "http_route": "POST /management/v1/warehouse", "status_code": 201, "warehouse_id": null, "warehouse_name": null, "created_at": "00:16:32", "updated_at": null},
    ///                                                  {"count": 1, "http_route": "POST /catalog/v1/{prefix}/namespaces/{namespace}/tables", "status_code": 201, "warehouse_id": "ff17f1d0-90ad-4e7d-bf02-be718b78c2ee", "warehouse_name": "staging", "created_at": "00:30:00", "updated_at": null}]]
    /// - 00:45:00: table created:
    ///     - timestamps: ["01:00:00"], called_endpoints: [[{"count": 1, "http_route": "POST /management/v1/warehouse", "status_code": 201, "warehouse_id": null, "warehouse_name": null, "created_at": "00:16:32", "updated_at": null},
    ///                                                  {"count": 1, "http_route": "POST /catalog/v1/{prefix}/namespaces/{namespace}/tables", "status_code": 201, "warehouse_id": "ff17f1d0-90ad-4e7d-bf02-be718b78c2ee", "warehouse_name": "staging", "created_at": "00:30:00", "updated_at": "00:45:00"}]]
    /// - 01:00:36: table deleted:
    ///     - timestamps: ["01:00:00","02:00:00"], called_endpoints: [[{"count": 1, "http_route": "POST /management/v1/warehouse", "status_code": 201, "warehouse_id": null, "warehouse_name": null, "created_at": "00:16:32", "updated_at": null},
    ///                                                  {"count": 1, "http_route": "POST /catalog/v1/{prefix}/namespaces/{namespace}/tables", "status_code": 201, "warehouse_id": "ff17f1d0-90ad-4e7d-bf02-be718b78c2ee", "warehouse_name": "staging", "created_at": "00:30:00", "updated_at": "00:45:00"}],
    ///                                                   [{"count": 1, "http_route": "DELETE /catalog/v1/{prefix}/namespaces/{namespace}/tables/{table}", "status_code": 200, "warehouse_id": "ff17f1d0-90ad-4e7d-bf02-be718b78c2ee", "warehouse_name": "staging", "created_at": "01:00:36", "updated_at": "null"}]]
    #[utoipa::path(
        post,
        tag = "project",
        path = ManagementV1Endpoint::LoadEndpointStatistics.path(),
        request_body = GetEndpointStatisticsRequest,
        responses(
            (status = 200, description = "Endpoint statistics", body = EndpointStatisticsResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_endpoint_statistics<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(query): Json<GetEndpointStatisticsRequest>,
    ) -> Result<Json<EndpointStatisticsResponse>> {
        ApiServer::<C, A, S>::get_endpoint_statistics(api_context, query, metadata)
            .await
            .map(Json)
    }

    /// List soft-deleted tabulars
    ///
    /// List all soft-deleted tabulars in the warehouse that are visible to you.
    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::ListDeletedTabulars.path(),
        params(("warehouse_id" = Uuid,), ListDeletedTabularsQuery),
        responses(
            (status = 200, description = "List of soft-deleted tabulars", body = ListDeletedTabularsResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn list_deleted_tabulars<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        Query(query): Query<ListDeletedTabularsQuery>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
    ) -> Result<Json<ListDeletedTabularsResponse>> {
        ApiServer::<C, A, S>::list_soft_deleted_tabulars(
            warehouse_id.into(),
            query,
            api_context,
            metadata,
        )
        .await
        .map(Json)
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::UndropTabularsDeprecated.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 204, description = "Tabular undropped successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    #[deprecated(
        since = "0.7.0",
        note = "This endpoint is deprecated and will be removed soon, please use /management/v1/warehouse/{warehouse_id}/deleted-tabulars/undrop instead."
    )]
    async fn undrop_tabulars_deprecated<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UndropTabularsRequest>,
    ) -> Result<StatusCode> {
        ApiServer::<C, A, S>::undrop_tabulars(
            WarehouseIdent::from(warehouse_id),
            metadata,
            request,
            api_context,
        )
        .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::UndropTabulars.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 204, description = "Tabular undropped successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn undrop_tabulars<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Extension(metadata): Extension<RequestMetadata>,
        Json(request): Json<UndropTabularsRequest>,
    ) -> Result<StatusCode> {
        ApiServer::<C, A, S>::undrop_tabulars(
            WarehouseIdent::from(warehouse_id),
            metadata,
            request,
            api_context,
        )
        .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    #[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
    pub struct ProtectionResponse {
        /// Indicates whether the entity is protected
        pub protected: bool,
        /// Updated at
        pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    impl IntoResponse for ProtectionResponse {
        fn into_response(self) -> Response {
            (StatusCode::OK, Json(self)).into_response()
        }
    }

    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::GetTableProtection.path(),
        params(("warehouse_id" = Uuid,),("table_id" = Uuid,)),
        responses(
            (status = 200, body =  ProtectionResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_table_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, table_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::get_table_protection(
            TableIdentUuid::from(table_id),
            warehouse_id.into(),
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::SetTableProtection.path(),
        params(("warehouse_id" = Uuid,),("table_id" = Uuid,)),
        responses(
            (status = 200, body =  ProtectionResponse, description = "Table protection set successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn set_table_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, table_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Json(SetProtectionRequest { protected }): Json<SetProtectionRequest>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::set_table_protection(
            TableIdentUuid::from(table_id),
            warehouse_id.into(),
            protected,
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::GetViewProtection.path(),
        params(("warehouse_id" = Uuid,),("view_id" = Uuid,)),
        responses(
            (status = 200, body = ProtectionResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_view_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, view_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::get_view_protection(
            ViewIdentUuid::from(view_id),
            warehouse_id.into(),
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::SetViewProtection.path(),
        params(("warehouse_id" = Uuid,),("view_id" = Uuid,)),
        responses(
            (status = 200, body = ProtectionResponse, description = "View protection set successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn set_view_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, view_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Json(SetProtectionRequest { protected }): Json<SetProtectionRequest>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::set_view_protection(
            ViewIdentUuid::from(view_id),
            warehouse_id.into(),
            protected,
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        get,
        tag = "warehouse",
        path = ManagementV1Endpoint::GetNamespaceProtection.path(),
        params(("warehouse_id" = Uuid,),("namespace_id" = Uuid,)),
        responses(
            (status = 200, body = ProtectionResponse),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn get_namespace_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, namespace_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::get_namespace_protection(
            NamespaceIdentUuid::from(namespace_id),
            warehouse_id.into(),
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::SetNamespaceProtection.path(),
        params(("warehouse_id" = Uuid,),("namespace_id" = Uuid,)),
        responses(
            (status = 200, body = ProtectionResponse, description = "Namespace protection set successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn set_namespace_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path((warehouse_id, namespace_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Json(SetProtectionRequest { protected }): Json<SetProtectionRequest>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::set_namespace_protection(
            NamespaceIdentUuid::from(namespace_id),
            warehouse_id.into(),
            protected,
            api_context,
            metadata,
        )
        .await
    }

    #[utoipa::path(
        post,
        tag = "warehouse",
        path = ManagementV1Endpoint::SetWarehouseProtection.path(),
        params(("warehouse_id" = Uuid,)),
        responses(
            (status = 200, body = ProtectionResponse, description = "Warehouse protection set successfully"),
            (status = "4XX", body = IcebergErrorResponse),
        )
    )]
    async fn set_warehouse_protection<C: Catalog, A: Authorizer + Clone, S: SecretStore>(
        Path(warehouse_id): Path<uuid::Uuid>,
        Extension(metadata): Extension<RequestMetadata>,
        AxumState(api_context): AxumState<ApiContext<State<A, C, S>>>,
        Json(SetProtectionRequest { protected }): Json<SetProtectionRequest>,
    ) -> Result<ProtectionResponse> {
        ApiServer::<C, A, S>::set_warehouse_protection(
            warehouse_id.into(),
            protected,
            api_context,
            metadata,
        )
        .await
    }

    #[derive(Debug, Serialize, utoipa::ToSchema)]
    #[serde(rename_all = "kebab-case")]
    pub struct ListDeletedTabularsResponse {
        /// List of tabulars
        pub tabulars: Vec<DeletedTabularResponse>,
        /// Token to fetch the next page
        pub next_page_token: Option<String>,
    }

    #[derive(Debug, Serialize, utoipa::ToSchema)]
    #[serde(rename_all = "kebab-case")]
    pub struct DeletedTabularResponse {
        /// Unique identifier of the tabular
        pub id: uuid::Uuid,
        /// Name of the tabular
        pub name: String,
        /// List of namespace parts the tabular belongs to
        pub namespace: Vec<String>,
        /// Type of the tabular
        pub typ: TabularType,
        /// Warehouse ID where the tabular is stored
        #[schema(value_type = uuid::Uuid)]
        pub warehouse_id: WarehouseIdent,
        /// Date when the tabular was created
        pub created_at: chrono::DateTime<chrono::Utc>,
        /// Date when the tabular was deleted
        pub deleted_at: chrono::DateTime<chrono::Utc>,
        /// Date when the tabular will not be recoverable anymore
        pub expiration_date: chrono::DateTime<chrono::Utc>,
    }

    impl From<TabularIdentUuid> for TabularType {
        fn from(ident: TabularIdentUuid) -> Self {
            match ident {
                TabularIdentUuid::Table(_) => TabularType::Table,
                TabularIdentUuid::View(_) => TabularType::View,
            }
        }
    }

    /// Type of tabular
    #[derive(Debug, Serialize, Clone, Copy, utoipa::ToSchema, strum::Display, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum TabularType {
        Table,
        View,
    }

    #[derive(Debug, Serialize, utoipa::ToSchema, Clone, Copy, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum DeleteKind {
        Default,
        Purge,
    }

    #[must_use]
    pub fn api_doc<A: Authorizer>() -> utoipa::openapi::OpenApi {
        let mut doc = ManagementApiDoc::openapi();
        doc.merge(A::api_doc());
        doc
    }

    impl<C: Catalog, A: Authorizer, S: SecretStore> ApiServer<C, A, S> {
        pub fn new_v1_router(authorizer: &A) -> Router<ApiContext<State<A, C, S>>> {
            Router::new()
                // Server
                .route("/info", get(get_server_info))
                .route("/bootstrap", post(bootstrap))
                .route("/endpoint-statistics", post(get_endpoint_statistics))
                // Role management
                .route("/role", get(list_roles).post(create_role))
                .route(
                    "/role/{role_id}",
                    get(get_role).post(update_role).delete(delete_role),
                )
                .route("/search/role", post(search_role))
                // User management
                .route("/whoami", get(whoami))
                .route("/search/user", post(search_user))
                .route(
                    "/user/{user_id}",
                    get(get_user).put(update_user).delete(delete_user),
                )
                .route("/user", get(list_user).post(create_user))
                // Default project
                .route(
                    "/default-project",
                    get(get_default_project_deprecated).delete(delete_default_project_deprecated),
                )
                .route(
                    "/default-project/rename",
                    post(rename_default_project_deprecated),
                )
                .route("/project/rename", post(rename_default_project))
                // Create a new project
                .route(
                    "/project",
                    post(create_project)
                        .get(get_default_project)
                        .delete(delete_default_project),
                )
                .route(
                    "/project/{project_id}",
                    get(get_project_by_id).delete(delete_project_by_id),
                )
                .route("/project/{project_id}/rename", post(rename_project_by_id))
                // Create a new warehouse
                .route("/warehouse", post(create_warehouse).get(list_warehouses))
                // List all projects
                .route("/project-list", get(list_projects))
                .route(
                    "/warehouse/{warehouse_id}",
                    get(get_warehouse).delete(delete_warehouse),
                )
                // Rename warehouse
                .route("/warehouse/{warehouse_id}/rename", post(rename_warehouse))
                // Deactivate warehouse
                .route(
                    "/warehouse/{warehouse_id}/deactivate",
                    post(deactivate_warehouse),
                )
                .route(
                    "/warehouse/{warehouse_id}/activate",
                    post(activate_warehouse),
                )
                // Update storage profile and credential.
                // The old credential is not re-used. If credentials are not provided,
                // we assume that this endpoint does not require a secret.
                .route(
                    "/warehouse/{warehouse_id}/storage",
                    post(update_storage_profile),
                )
                // Update only the storage credential - keep the storage profile as is
                .route(
                    "/warehouse/{warehouse_id}/storage-credential",
                    post(update_storage_credential),
                )
                // Get warehouse statistics
                .route(
                    "/warehouse/{warehouse_id}/statistics",
                    get(get_warehouse_statistics),
                )
                .route(
                    "/warehouse/{warehouse_id}/deleted-tabulars",
                    get(list_deleted_tabulars),
                )
                .route(
                    "/warehouse/{warehouse_id}/deleted_tabulars/undrop",
                    #[allow(deprecated)]
                    post(undrop_tabulars_deprecated),
                )
                .route(
                    "/warehouse/{warehouse_id}/deleted-tabulars/undrop",
                    post(undrop_tabulars),
                )
                .route(
                    "/warehouse/{warehouse_id}/delete-profile",
                    post(update_warehouse_delete_profile),
                )
                .route(
                    "/warehouse/{warehouse_id}/table/{table_id}/protection",
                    get(get_table_protection).post(set_table_protection),
                )
                .route(
                    "/warehouse/{warehouse_id}/view/{view_id}/protection",
                    get(get_view_protection).post(set_view_protection),
                )
                .route(
                    "/warehouse/{warehouse_id}/namespace/{namespace_id}/protection",
                    get(get_namespace_protection).post(set_namespace_protection),
                )
                .route(
                    "/warehouse/{warehouse_id}/protection",
                    post(set_warehouse_protection),
                )
                .merge(authorizer.new_router())
        }
    }
}
