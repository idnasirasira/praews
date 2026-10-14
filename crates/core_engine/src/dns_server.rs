use anyhow::{Context, Result};
use async_trait::async_trait;
use hickory_proto::op::{Header, Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};
use hickory_proto::serialize::binary::BinDecodable;
use hickory_server::authority::MessageResponseBuilder;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::ServerFuture;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::str::FromStr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

/// Detect the active LAN IPv4 without sending real traffic.
pub fn get_local_lan_ip() -> Ipv4Addr {
    if let Ok(socket) = StdUdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                if let IpAddr::V4(ipv4) = local_addr.ip() {
                    return ipv4;
                }
            }
        }
    }
    Ipv4Addr::new(127, 0, 0, 1)
}

#[derive(Clone)]
pub struct NamoDnsHandler {
    tld: String,
    lan_ip: Ipv4Addr,
    upstream_dns: SocketAddr,
}

impl NamoDnsHandler {
    pub fn new(tld: &str) -> Self {
        let normalized_tld = format!(".{}.", tld.trim_matches('.'));
        Self {
            tld: normalized_tld,
            lan_ip: get_local_lan_ip(),
            upstream_dns: SocketAddr::from_str("1.1.1.1:53").expect("valid upstream"),
        }
    }

    async fn forward_to_upstream(&self, request: &Request) -> Option<Message> {
        let mut outbound = Message::new();
        outbound.set_id(request.id());
        outbound.set_message_type(MessageType::Query);
        outbound.set_op_code(OpCode::Query);
        outbound.set_recursion_desired(true);
        outbound.add_query(request.query().original().clone());
        let bytes = outbound.to_vec().ok()?;

        let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
        socket.send_to(&bytes, self.upstream_dns).await.ok()?;
        let mut buf = [0u8; 4096];
        let (len, _) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
            .await
            .ok()?
            .ok()?;
        Message::from_bytes(&buf[..len]).ok()
    }
}

#[async_trait]
impl RequestHandler for NamoDnsHandler {
    async fn handle_request<R: ResponseHandler>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let builder = MessageResponseBuilder::from_message_request(request);
        let mut header = Header::response_from_request(request.header());
        header.set_recursion_available(true);

        let query = request.query();
        let query_name = query.name().to_string();

        if query_name.ends_with(&self.tld) {
            header.set_authoritative(true);

            if query.query_type() == RecordType::A {
                let target_ip = if request.src().ip().is_loopback() {
                    Ipv4Addr::new(127, 0, 0, 1)
                } else {
                    self.lan_ip
                };

                let rdata = RData::A(A(target_ip));
                let mut record = Record::from_rdata(Name::from(query.name()), 60, rdata);
                record.set_dns_class(DNSClass::IN);

                let response = builder.build(header, std::iter::once(&record), &[], &[], &[]);
                return response_handle
                    .send_response(response)
                    .await
                    .unwrap_or_else(|_| Header::new().into());
            }

            let response = builder.build_no_records(header);
            return response_handle
                .send_response(response)
                .await
                .unwrap_or_else(|_| Header::new().into());
        }

        if let Some(upstream_msg) = self.forward_to_upstream(request).await {
            let answers = upstream_msg.answers();
            let name_servers = upstream_msg.name_servers();
            let additionals = upstream_msg.additionals();
            header.set_response_code(upstream_msg.response_code());
            let response = builder.build(header, answers, name_servers, &[], additionals);
            return response_handle
                .send_response(response)
                .await
                .unwrap_or_else(|_| Header::new().into());
        }

        header.set_response_code(ResponseCode::ServFail);
        let response = builder.build_no_records(header);
        response_handle
            .send_response(response)
            .await
            .unwrap_or_else(|_| Header::new().into())
    }
}

/// Controller for starting/stopping the local NAMO-style DNS server.
pub struct LocalDnsServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    pub active_lan_ip: Ipv4Addr,
    pub port: u16,
}

impl LocalDnsServer {
    pub fn new(port: u16) -> Self {
        Self {
            shutdown_tx: None,
            active_lan_ip: get_local_lan_ip(),
            port,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.shutdown_tx.is_some() {
            return Ok(());
        }

        let bind_addr = format!("0.0.0.0:{}", self.port);
        let udp_socket = UdpSocket::bind(&bind_addr)
            .await
            .with_context(|| format!("failed to bind DNS UDP on {bind_addr}"))?;

        let handler = NamoDnsHandler::new("test");
        self.active_lan_ip = handler.lan_ip;

        let mut server = ServerFuture::new(handler);
        server.register_socket(udp_socket);

        let (tx, rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(tx);

        tokio::spawn(async move {
            tokio::select! {
                _ = server.block_until_done() => {},
                _ = rx => {
                    let _ = server.shutdown_gracefully().await;
                }
            }
        });

        tracing::info!(
            port = self.port,
            lan_ip = %self.active_lan_ip,
            "local DNS server started (*.test)"
        );
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    pub fn mobile_hint(&self) -> String {
        format!(
            "On your phone Wi-Fi DNS settings, set DNS to {} (this machine). Then open http://myapp.test",
            self.active_lan_ip
        )
    }

    pub fn sslip_url(slug: &str, lan_ip: Ipv4Addr) -> String {
        format!("http://{slug}.{lan_ip}.sslip.io")
    }
}

