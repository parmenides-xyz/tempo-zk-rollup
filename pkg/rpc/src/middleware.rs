use std::future::{ready, Ready};

use crate::error::HTTPError;
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, ResponseError,
};
use futures_util::future::LocalBoxFuture;
use tracing::error;

pub struct Middleware;

impl<S, B> Transform<S, ServiceRequest> for Middleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = MiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(MiddlewareService { service }))
    }
}

pub struct MiddlewareService<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for MiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let path = req.path().to_owned();
        let method = req.method().to_owned().as_str().to_owned();
        let fut = self.service.call(req);

        Box::pin(async move {
            match fut.await {
                Ok(res) => {
                    if let Some(err) = res.response().error() {
                        if let Some(err) = err.as_error::<HTTPError>() {
                            let status_code = err.status_code().as_u16();

                            // send 5XX errors to Sentry
                            if status_code >= 500 {
                                sentry::capture_error(&err);
                            }

                            error!(
                                path,
                                method,
                                ?err,
                                code = err.code.to_string(),
                                data = ?err.data,
                                report = err.report(),
                                is_request_error = true, // for filtering in sentry_layer
                                "HTTP Error"
                            );
                        } else {
                            error!(path, method, ?err, "Unhandled server error");
                        }
                    }

                    Ok(res)
                }
                Err(err) => {
                    error!(path, method, ?err, "Unhandled server error");
                    Err(err)
                }
            }
        })
    }
}
