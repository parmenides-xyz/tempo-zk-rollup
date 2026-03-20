use crate::code::ErrorCode;
use actix_web::Responder;
use actix_web::ResponseError;
use actix_web::{http::header::ContentType, HttpResponse};
use serde::Serialize;
use std::fmt::Debug;
use std::{error::Error, fmt::Display};

pub type HttpResult<T> = std::result::Result<T, HTTPError>;

#[derive(Debug)]
pub struct HTTPError {
    pub code: ErrorCode,
    pub reason: String,
    source: Option<Box<dyn Error>>,
    pub data: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct ErrorOutput {
    error: ErrorDetail,
}

#[derive(Serialize)]
pub struct ErrorDetail {
    code: String,
    reason: String,
    message: String,
    data: Option<serde_json::Value>,
}
impl HTTPError {
    pub fn new(
        code: ErrorCode,
        reason: &str,
        source: Option<Box<dyn std::error::Error>>,
        data: Option<impl Serialize>,
    ) -> HTTPError {
        HTTPError {
            data: data.map(|data| {
                serde_json::to_value(data).unwrap_or_else(|err| {
                    panic!(
                        "Unable to serialize error data for error {source:?}, serializing data err: {err:?}",
                    )
                })
            }),
            reason: reason.to_owned(),
            code,
            source,
        }
    }

    pub fn internal(err: Box<dyn std::error::Error>) -> HTTPError {
        Self::new(ErrorCode::Internal, "internal", Some(err), None::<()>)
    }

    pub fn bad_request(
        reason: &str,
        source: Option<Box<dyn std::error::Error>>,
        data: Option<impl Serialize>,
    ) -> HTTPError {
        Self::new(ErrorCode::BadRequest, reason, source, data)
    }

    pub fn permission_denied() -> HTTPError {
        Self::new(
            ErrorCode::PermissionDenied,
            "permission-denied",
            None,
            None::<()>,
        )
    }

    /// Get all of the sources of the error
    pub fn sources(&self) -> Vec<&dyn std::error::Error> {
        let mut sources = Vec::new();
        let mut error: &dyn std::error::Error = self;
        while let Some(source) = error.source() {
            sources.push(source.to_owned());
            error = source;
        }
        sources
    }

    /// Get a full report of the error
    pub fn report(&self) -> String {
        let err = self;
        let mut output: String = self.message();

        // Log out each source error
        let mut error: &dyn std::error::Error = err;
        while let Some(source) = error.source() {
            output = format!("{output}\n  Caused by: {source}");
            error = source;
        }
        output
    }

    pub fn message(&self) -> String {
        self.source
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

impl Display for HTTPError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} -> {}", self.code, self.message())
    }
}

impl std::error::Error for HTTPError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|e| e.as_ref())
    }
}

impl actix_web::error::ResponseError for HTTPError {
    fn error_response(&self) -> HttpResponse {
        let error = ErrorOutput {
            error: ErrorDetail {
                code: self.code.to_string(),
                reason: self.reason.clone(),
                message: self.message(),
                data: self.data.clone(),
            },
        };
        #[allow(clippy::unwrap_used)]
        HttpResponse::build(self.status_code())
            .insert_header(ContentType::json())
            .body(serde_json::to_string(&error).unwrap())
    }

    fn status_code(&self) -> actix_web::http::StatusCode {
        self.code.status_code()
    }
}

impl From<eyre::Error> for HTTPError {
    fn from(err: eyre::Error) -> Self {
        HTTPError::new(
            ErrorCode::Internal,
            "internal-error",
            Some(err.into()),
            None::<serde_json::Value>,
        )
    }
}

pub async fn not_found_error_handler() -> impl Responder {
    let error = HTTPError::new(
        ErrorCode::NotFound, // Assuming you have this variant defined.
        "not-found",
        None,       // No other error caused this error.
        None::<()>, // No extra data.
    );
    error.error_response() // Returns HttpResponse with JSON error.
}
