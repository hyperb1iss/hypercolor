//! Browser-neutral HTTP transport contract for daemon API calls.

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMultipartPart {
    pub name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpRequestBody {
    Empty,
    Bytes(Vec<u8>),
    Multipart(Vec<HttpMultipartPart>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub headers: Vec<HttpHeader>,
    pub body: HttpRequestBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransportError {
    pub message: String,
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpTransportError {}

pub type HttpTransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, HttpTransportError>> + 'a>>;

pub trait HttpTransport {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_>;
}
