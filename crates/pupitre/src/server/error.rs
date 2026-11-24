// SPDX-License-Identifier: MIT OR Apache-2.0
//! The errors of the API (5.4): the closed list of codes, their statuses, their fixed messages
//! and their extra members. An error body never holds a value of the request, a detail of the
//! PDP, a path of the server or a secret.

use bytes::Bytes;
use http::StatusCode;
use pupitre_core::error::{FieldError, errors_json};
use pupitre_core::json::{self, Json};

/// A code of 5.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// `400 bad_request`
    BadRequest,
    /// `400 invalid_options_request` (3.16)
    InvalidOptionsRequest,
    /// `401 unauthenticated` (3.4)
    Unauthenticated,
    /// `403 forbidden` (3.21)
    Forbidden,
    /// `403 approval_cannot_wait` (3.19)
    ApprovalCannotWait,
    /// `403 ceiling_exceeded` (3.10)
    CeilingExceeded,
    /// `403 apply_disabled` (3.22)
    ApplyDisabled,
    /// `403 cross_site_request` (7.4)
    CrossSiteRequest,
    /// `404 not_found`
    NotFound,
    /// `421 misdirected_request` (7.4)
    MisdirectedRequest,
    /// `405 method_not_allowed` (5.3)
    MethodNotAllowed,
    /// `408 request_timeout` (7.10)
    RequestTimeout,
    /// `409 conflict` (2.20)
    Conflict,
    /// `409 already_exists` (2.20)
    AlreadyExists,
    /// `409 in_use` (2.16)
    InUse,
    /// `409 not_empty` (2.16)
    NotEmpty,
    /// `409 check_required` (4.24)
    CheckRequired,
    /// `409 check_used` (4.24)
    CheckUsed,
    /// `409 check_too_old` (4.24)
    CheckTooOld,
    /// `409 run_finished` (4.27)
    RunFinished,
    /// `413 payload_too_large` (7.10)
    PayloadTooLarge,
    /// `414 uri_too_long` (7.10)
    UriTooLong,
    /// `415 unsupported_media_type` (7.4)
    UnsupportedMediaType,
    /// `422 invalid_document` (2.7 to 2.16)
    InvalidDocument,
    /// `422 invalid_secret` (2.24, 2.25)
    InvalidSecret,
    /// `422 invalid_run_request` (4.4)
    InvalidRunRequest,
    /// `422 limit_exceeded` (2.42)
    LimitExceeded,
    /// `428 precondition_required` (5.5)
    PreconditionRequired,
    /// `431 headers_too_large` (7.10)
    HeadersTooLarge,
    /// `500 internal_error`
    InternalError,
    /// `503 server_busy` (7.10)
    ServerBusy,
    /// `503 pdp_unavailable` (3.18)
    PdpUnavailable,
    /// `503 store_unavailable`
    StoreUnavailable,
    /// `503 store_over_limit` (2.41): the store holds more than a limit. Not retryable.
    StoreOverLimit,
    /// `503 run_queue_full` (4.5)
    RunQueueFull,
}

impl Code {
    /// The codes of 5.4, in the order of its table.
    pub const ALL: [Code; 35] = [
        Code::BadRequest,
        Code::InvalidOptionsRequest,
        Code::Unauthenticated,
        Code::Forbidden,
        Code::ApprovalCannotWait,
        Code::CeilingExceeded,
        Code::ApplyDisabled,
        Code::CrossSiteRequest,
        Code::NotFound,
        Code::MisdirectedRequest,
        Code::MethodNotAllowed,
        Code::RequestTimeout,
        Code::Conflict,
        Code::AlreadyExists,
        Code::InUse,
        Code::NotEmpty,
        Code::CheckRequired,
        Code::CheckUsed,
        Code::CheckTooOld,
        Code::RunFinished,
        Code::PayloadTooLarge,
        Code::UriTooLong,
        Code::UnsupportedMediaType,
        Code::InvalidDocument,
        Code::InvalidSecret,
        Code::InvalidRunRequest,
        Code::LimitExceeded,
        Code::PreconditionRequired,
        Code::HeadersTooLarge,
        Code::InternalError,
        Code::ServerBusy,
        Code::PdpUnavailable,
        Code::StoreUnavailable,
        Code::StoreOverLimit,
        Code::RunQueueFull,
    ];

    /// The text of the member `error`.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::BadRequest => "bad_request",
            Code::InvalidOptionsRequest => "invalid_options_request",
            Code::Unauthenticated => "unauthenticated",
            Code::Forbidden => "forbidden",
            Code::ApprovalCannotWait => "approval_cannot_wait",
            Code::CeilingExceeded => "ceiling_exceeded",
            Code::ApplyDisabled => "apply_disabled",
            Code::CrossSiteRequest => "cross_site_request",
            Code::NotFound => "not_found",
            Code::MisdirectedRequest => "misdirected_request",
            Code::MethodNotAllowed => "method_not_allowed",
            Code::RequestTimeout => "request_timeout",
            Code::Conflict => "conflict",
            Code::AlreadyExists => "already_exists",
            Code::InUse => "in_use",
            Code::NotEmpty => "not_empty",
            Code::CheckRequired => "check_required",
            Code::CheckUsed => "check_used",
            Code::CheckTooOld => "check_too_old",
            Code::RunFinished => "run_finished",
            Code::PayloadTooLarge => "payload_too_large",
            Code::UriTooLong => "uri_too_long",
            Code::UnsupportedMediaType => "unsupported_media_type",
            Code::InvalidDocument => "invalid_document",
            Code::InvalidSecret => "invalid_secret",
            Code::InvalidRunRequest => "invalid_run_request",
            Code::LimitExceeded => "limit_exceeded",
            Code::PreconditionRequired => "precondition_required",
            Code::HeadersTooLarge => "headers_too_large",
            Code::InternalError => "internal_error",
            Code::ServerBusy => "server_busy",
            Code::PdpUnavailable => "pdp_unavailable",
            Code::StoreUnavailable => "store_unavailable",
            Code::StoreOverLimit => "store_over_limit",
            Code::RunQueueFull => "run_queue_full",
        }
    }

    /// The HTTP status of the code (5.4).
    pub fn status(self) -> StatusCode {
        match self {
            Code::BadRequest | Code::InvalidOptionsRequest => StatusCode::BAD_REQUEST,
            Code::Unauthenticated => StatusCode::UNAUTHORIZED,
            Code::Forbidden
            | Code::ApprovalCannotWait
            | Code::CeilingExceeded
            | Code::ApplyDisabled
            | Code::CrossSiteRequest => StatusCode::FORBIDDEN,
            Code::NotFound => StatusCode::NOT_FOUND,
            Code::MisdirectedRequest => StatusCode::MISDIRECTED_REQUEST,
            Code::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Code::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Code::Conflict
            | Code::AlreadyExists
            | Code::InUse
            | Code::NotEmpty
            | Code::CheckRequired
            | Code::CheckUsed
            | Code::CheckTooOld
            | Code::RunFinished => StatusCode::CONFLICT,
            Code::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Code::UriTooLong => StatusCode::URI_TOO_LONG,
            Code::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Code::InvalidDocument | Code::InvalidSecret | Code::InvalidRunRequest | Code::LimitExceeded => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Code::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
            Code::HeadersTooLarge => StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            Code::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            Code::ServerBusy
            | Code::PdpUnavailable
            | Code::StoreUnavailable
            | Code::StoreOverLimit
            | Code::RunQueueFull => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The fixed English text of the member `message` (5.4).
    pub fn message(self) -> &'static str {
        match self {
            Code::BadRequest => "The request does not have the shape of its route.",
            Code::InvalidOptionsRequest => "The query string of the options request is not valid.",
            Code::Unauthenticated => "A valid identity token is necessary.",
            Code::Forbidden => "You have no right for this operation.",
            Code::ApprovalCannotWait => "This operation needs an approval and cannot wait for it.",
            Code::CeilingExceeded => "The run reaches more hosts than the ceiling permits.",
            Code::ApplyDisabled => "An apply and a command are off in the mode open.",
            Code::CrossSiteRequest => "The request comes from another site.",
            Code::NotFound => "No such item.",
            Code::MisdirectedRequest => "The header Host is not in the list of hosts that the server serves.",
            Code::MethodNotAllowed => "The route does not accept this method.",
            Code::RequestTimeout => "The body did not arrive in time.",
            Code::Conflict => "The document changed after your read.",
            Code::AlreadyExists => "The document already exists.",
            Code::InUse => "Another document uses this item.",
            Code::NotEmpty => "The item still holds other documents.",
            Code::CheckRequired => "The check run is not a check run that passed.",
            Code::CheckUsed => "Another apply already uses this check run.",
            Code::CheckTooOld => "The check run is too old.",
            Code::RunFinished => "The run is already finished.",
            Code::PayloadTooLarge => "The body is larger than the limit.",
            Code::UriTooLong => "The request target is longer than 8,192 bytes.",
            Code::UnsupportedMediaType => "The body must have the media type application/json.",
            Code::InvalidDocument => "The document is not valid.",
            Code::InvalidSecret => "The secret is not valid.",
            Code::InvalidRunRequest => "The run request is not valid.",
            Code::LimitExceeded => "The write goes above a limit of the store.",
            Code::PreconditionRequired => "The request needs one precondition header.",
            Code::HeadersTooLarge => "The headers are larger than the limit.",
            Code::InternalError => "The server has a defect. The log has the details.",
            Code::ServerBusy => "The server is busy. Try again.",
            Code::PdpUnavailable => "The decision service did not answer in time.",
            Code::StoreUnavailable => "The store did not answer, or the catalog is not ready.",
            Code::StoreOverLimit => "The store holds more than a limit. Delete documents, or ask the operator.",
            Code::RunQueueFull => "The queue of runs is full.",
        }
    }

    /// True for the codes of 503, except `store_over_limit` (5.4). A try again cannot help while
    /// the store holds more than a limit: a person must reduce the count.
    pub fn retryable(self) -> bool {
        self.status() == StatusCode::SERVICE_UNAVAILABLE && self != Code::StoreOverLimit
    }

    /// The value of `Retry-After` in seconds (7.10, 4.5).
    pub fn retry_after(self) -> Option<u64> {
        match self {
            Code::ServerBusy => Some(1),
            Code::RunQueueFull => Some(5),
            _ => None,
        }
    }
}

/// An error answer: a code and its extra members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The code.
    pub code: Code,
    extra: Vec<(&'static str, Json)>,
    /// The value of the header `Allow` of a `405`.
    pub allow: Option<String>,
    /// True when the connection closes after the answer.
    pub close: bool,
}

impl ApiError {
    /// An error with no extra member.
    pub fn new(code: Code) -> ApiError {
        ApiError { code, extra: Vec::new(), allow: None, close: false }
    }

    /// `400 bad_request`
    pub fn bad_request() -> ApiError {
        ApiError::new(Code::BadRequest)
    }

    /// `404 not_found`
    pub fn not_found() -> ApiError {
        ApiError::new(Code::NotFound)
    }

    /// `403 forbidden` with the use action (3.21).
    pub fn forbidden(action: &str) -> ApiError {
        ApiError::new(Code::Forbidden).with("action", Json::str(action))
    }

    /// `405 method_not_allowed` with the header `Allow`.
    pub fn method_not_allowed(allow: String) -> ApiError {
        let mut e = ApiError::new(Code::MethodNotAllowed);
        e.allow = Some(allow);
        e
    }

    /// An error with a list `errors` (5.4).
    pub fn with_errors(code: Code, errors: &[FieldError]) -> ApiError {
        ApiError::new(code).with("errors", errors_json(errors))
    }

    /// `409 conflict`, with the current etag when the document has one.
    pub fn conflict(current: Option<&pupitre_core::etag::Etag>) -> ApiError {
        let e = ApiError::new(Code::Conflict);
        match current {
            Some(etag) => e.with("etag", Json::str(etag.to_hex())),
            None => e,
        }
    }

    /// `422 limit_exceeded` with the key of the limit.
    pub fn limit_exceeded(limit: &str) -> ApiError {
        ApiError::new(Code::LimitExceeded).with("limit", Json::str(limit))
    }

    /// `503 store_over_limit` with the key of the limit (2.41).
    pub fn store_over_limit(limit: &str) -> ApiError {
        ApiError::new(Code::StoreOverLimit).with("limit", Json::str(limit))
    }

    /// Add one extra member.
    #[must_use]
    pub fn with(mut self, name: &'static str, value: Json) -> ApiError {
        self.extra.push((name, value));
        self
    }

    /// Close the connection after the answer.
    #[must_use]
    pub fn closing(mut self) -> ApiError {
        self.close = true;
        self
    }

    /// The compact body (5.4). The body of `approval_cannot_wait` is exactly
    /// `{"error":"approval_cannot_wait"}` (P 17.7a).
    pub fn body(&self) -> Bytes {
        if self.code == Code::ApprovalCannotWait {
            return Bytes::from_static(br#"{"error":"approval_cannot_wait"}"#);
        }
        let mut pairs: Vec<(String, Json)> = vec![
            ("error".to_string(), Json::str(self.code.as_str())),
            ("message".to_string(), Json::str(self.code.message())),
            ("retryable".to_string(), Json::Bool(self.code.retryable())),
        ];
        for (name, value) in &self.extra {
            pairs.push(((*name).to_string(), value.clone()));
        }
        Bytes::from(json::to_compact(&Json::object(pairs)))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code.as_str())
    }
}

impl std::error::Error for ApiError {}
