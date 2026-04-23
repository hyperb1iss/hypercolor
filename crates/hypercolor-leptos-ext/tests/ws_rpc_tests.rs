#![cfg(feature = "ws-core")]

use bytes::{Bytes, BytesMut};
use hypercolor_leptos_ext::ws::transport::InMemoryTransport;
use hypercolor_leptos_ext::ws::{
    BinaryFrameDecode, BinaryFrameEncode, DecodeError, RPC_REQUEST_TAG, RPC_RESPONSE_TAG,
    RpcClient, RpcRequest, RpcResponse, RpcServer, RpcStatus, write_frame_prefix,
};

#[test]
fn rpc_request_roundtrips_method_and_payload() {
    let request = RpcRequest::new(42, "effects.apply", Bytes::from_static(b"payload"));

    let encoded = request.encode();

    assert_eq!(encoded[0], RPC_REQUEST_TAG);
    assert_eq!(RpcRequest::decode(&encoded), Ok(request));
}

#[test]
fn rpc_response_roundtrips_status_and_payload() {
    let response = RpcResponse::error(
        99,
        RpcStatus::NOT_FOUND,
        Bytes::from_static(b"missing effect"),
    );

    let encoded = response.encode();

    assert_eq!(encoded[0], RPC_RESPONSE_TAG);
    assert_eq!(RpcResponse::decode(&encoded), Ok(response));
}

#[test]
fn rpc_status_classifies_success_codes() {
    assert!(RpcStatus::OK.is_success());
    assert!(RpcStatus::from_code(204).is_success());
    assert!(!RpcStatus::BAD_REQUEST.is_success());
    assert!(!RpcStatus::INTERNAL_ERROR.is_success());
}

#[test]
fn rpc_request_rejects_invalid_method_utf8() {
    let mut encoded = BytesMut::new();
    write_frame_prefix::<RpcRequest>(&mut encoded);
    encoded.extend_from_slice(&1_u64.to_le_bytes());
    encoded.extend_from_slice(&1_u16.to_le_bytes());
    encoded.extend_from_slice(&[0xff]);

    assert_eq!(
        RpcRequest::decode(&encoded),
        Err(DecodeError::InvalidBody("method is not valid UTF-8"))
    );
}

#[test]
fn rpc_request_rejects_truncated_method() {
    let mut encoded = BytesMut::new();
    write_frame_prefix::<RpcRequest>(&mut encoded);
    encoded.extend_from_slice(&1_u64.to_le_bytes());
    encoded.extend_from_slice(&4_u16.to_le_bytes());
    encoded.extend_from_slice(b"ab");

    assert_eq!(RpcRequest::decode(&encoded), Err(DecodeError::Truncated));
}

#[tokio::test]
async fn rpc_client_and_server_exchange_raw_payloads() {
    let (client_transport, server_transport) = InMemoryTransport::pair();
    let mut client = RpcClient::new(client_transport);
    let mut server = RpcServer::new(server_transport);

    let client_fut = client.call_raw("effects.apply", Bytes::from_static(b"apply request"));
    let server_fut = async {
        let request = server
            .recv_request()
            .await
            .expect("request recv succeeds")
            .expect("request is present");
        assert_eq!(request.id, 1);
        assert_eq!(request.method, "effects.apply");
        assert_eq!(request.payload, Bytes::from_static(b"apply request"));

        server
            .send_response(RpcResponse::ok(
                request.id,
                Bytes::from_static(b"apply response"),
            ))
            .await
            .expect("response send succeeds");
    };

    let (response, ()) = futures_util::future::join(client_fut, server_fut).await;
    let response = response.expect("client call succeeds");

    assert_eq!(response.id, 1);
    assert_eq!(response.status, RpcStatus::OK);
    assert_eq!(response.payload, Bytes::from_static(b"apply response"));
}
