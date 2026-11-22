//! Resolve each download hop once, then pin the validated addresses in its HTTP client.
use super::public_ip;
use crate::error::{AppError, AppResult};
use reqwest::{header, Client};
use serde::Deserialize;
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLVER: &str = "cloudflare-dns.com";
const ENDPOINT: &str = "https://cloudflare-dns.com/dns-query";

fn unresolved() -> AppError {
    AppError::DownloadDns("未能获得下载主机的公网地址".into())
}

fn non_public() -> AppError {
    AppError::Denied("下载主机解析到非公网地址".into())
}

/// The benchmarking networks commonly returned by a Fake-IP/TUN resolver.
/// Never connect to these addresses, even when the original URL uses HTTPS.
fn synthetic(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => matches!(ip.octets(), [198, 18 | 19, _, _]),
        IpAddr::V6(ip) => ip.segments()[..3] == [0x2001, 2, 0],
    }
}

pub(super) async fn resolve(host: &str, port: u16) -> AppResult<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        // Loopback literals are used by offline fixtures only; a domain can never select loopback.
        return if public_ip(ip) || cfg!(test) && ip.is_loopback() {
            Ok(vec![SocketAddr::new(ip, port)])
        } else {
            Err(non_public())
        };
    }
    let system = tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| unresolved())?
        .map_err(|_| unresolved())?
        .collect();
    select_addresses(host, port, system, public_lookup(host)).await
}

async fn select_addresses(
    host: &str,
    port: u16,
    system: Vec<SocketAddr>,
    public_lookup: impl Future<Output = AppResult<Vec<IpAddr>>>,
) -> AppResult<Vec<SocketAddr>> {
    if system.is_empty() {
        return Err(unresolved());
    }
    if system.iter().all(|a| public_ip(a.ip())) {
        return Ok(system);
    }
    let name = host.trim_end_matches('.');
    // Do not forward local names, address literals, or genuine private/mixed DNS answers.
    if !system.iter().all(|a| synthetic(a.ip()))
        || !name.contains('.')
        || name.parse::<IpAddr>().is_ok()
        || matches!(
            name.rsplit('.').next(),
            Some(
                "local"
                    | "localhost"
                    | "localdomain"
                    | "lan"
                    | "internal"
                    | "corp"
                    | "home"
                    | "arpa"
                    | "onion"
                    | "invalid"
                    | "test"
                    | "example"
            )
        )
    {
        return Err(non_public());
    }
    let ips = public_lookup.await?;
    if ips.is_empty() {
        return Err(unresolved());
    }
    if !ips.iter().all(|&ip| public_ip(ip)) {
        return Err(non_public());
    }
    Ok(ips
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect())
}

async fn public_lookup(host: &str) -> AppResult<Vec<IpAddr>> {
    // Bootstrap without the DNS that supplied the placeholder. The URL hostname is
    // retained for SNI/certificate validation. Only a name and RR type leave this layer.
    let bootstrap = [Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(1, 0, 0, 1)]
        .map(|ip| SocketAddr::new(ip.into(), 443));
    let client = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LOOKUP_TIMEOUT)
        .resolve_to_addrs(RESOLVER, &bootstrap)
        .build()
        .map_err(|_| unresolved())?;
    let (mut v4, v6) = tokio::try_join!(query(&client, host, 1), query(&client, host, 28))?;
    v4.extend(v6);
    Ok(v4)
}

#[derive(Deserialize)]
struct Question {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
}
#[derive(Deserialize)]
struct Record {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
    data: String,
}
#[derive(Deserialize)]
struct DnsAnswer {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "TC")]
    truncated: bool,
    #[serde(rename = "Question")]
    questions: Vec<Question>,
    #[serde(rename = "Answer", default)]
    records: Vec<Record>,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim_end_matches('.')
        .eq_ignore_ascii_case(b.trim_end_matches('.'))
}

impl DnsAnswer {
    fn addresses(&self, host: &str, kind: u16) -> AppResult<Vec<IpAddr>> {
        if self.status != 0
            || self.truncated
            || !matches!(self.questions.as_slice(), [q] if q.kind==kind && same_name(&q.name,host))
        {
            return Err(unresolved());
        }
        // Only use records owned by the question or its CNAME chain. Ignore unrelated records.
        let mut owner = host;
        for _ in 0..16 {
            let mut aliases = self
                .records
                .iter()
                .filter(|r| r.kind == 5 && same_name(&r.name, owner));
            if let Some(cname) = aliases.next() {
                if aliases.next().is_some() || cname.data.is_empty() {
                    return Err(unresolved());
                }
                owner = &cname.data;
                continue;
            }
            return self
                .records
                .iter()
                .filter(|r| r.kind == kind && same_name(&r.name, owner))
                .map(|r| {
                    let ip: IpAddr = r.data.parse().map_err(|_| unresolved())?;
                    if (kind == 1 && ip.is_ipv4()) || (kind == 28 && ip.is_ipv6()) {
                        Ok(ip)
                    } else {
                        Err(unresolved())
                    }
                })
                .collect();
        }
        Err(unresolved())
    }
}

async fn query(client: &Client, host: &str, kind: u16) -> AppResult<Vec<IpAddr>> {
    let mut response = client
        .get(ENDPOINT)
        .header(header::ACCEPT, "application/dns-json")
        .query(&[
            ("name", host),
            ("type", if kind == 1 { "A" } else { "AAAA" }),
        ])
        .send()
        .await
        .map_err(|_| unresolved())?;
    if !response.status().is_success() {
        return Err(unresolved());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unresolved())? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(unresolved());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<DnsAnswer>(&bytes)
        .map_err(|_| unresolved())?
        .addresses(host, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn socket(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 443)
    }

    #[tokio::test]
    async fn fake_ip_recovery_pins_only_independently_resolved_public_addresses() {
        let local = vec![
            socket("198.18.0.110"),
            socket("2001:2::63"),
            socket("::ffff:198.19.1.2"),
        ];
        let expected = [socket("93.184.216.34"), socket("2606:4700::1111")];
        let addresses = select_addresses("www.glossmod.com", 443, local.clone(), async {
            Ok(expected.iter().map(|a| a.ip()).collect())
        })
        .await
        .unwrap();
        assert_eq!(addresses, expected);
        for ip in [
            "10.0.0.7",
            "127.0.0.1",
            "::1",
            "::ffff:169.254.169.254",
            "198.18.1.2",
            "2001:2::77",
        ] {
            let answer = async { Ok(vec![expected[0].ip(), socket(ip).ip()]) };
            assert!(matches!(
                select_addresses("files.example.org", 443, local.clone(), answer).await,
                Err(AppError::Denied(_))
            ));
        }
        assert!(matches!(
            select_addresses("files.example.org", 443, local, async { Ok(vec![]) }).await,
            Err(AppError::DownloadDns(_))
        ));
    }

    #[tokio::test]
    async fn public_private_and_mixed_local_answers_never_invoke_secondary_dns() {
        let never = || async { panic!("unexpected external DNS query") };
        let addr = socket("1.2.3.4");
        assert_eq!(
            select_addresses("files.example.org", 443, vec![addr], never())
                .await
                .unwrap(),
            [addr]
        );
        for (name, ips) in [
            ("files.example.org", vec!["10.0.0.1"]),
            ("files.example.org", vec!["1.2.3.4", "192.168.0.1"]),
            ("files.example.org", vec!["198.18.0.7", "fd00::1"]),
            ("files.example.org", vec!["2001:2:1::1"]),
            ("printer.home.arpa.", vec!["198.18.0.7"]),
            ("198.18.0.7", vec!["198.18.0.7"]),
        ] {
            assert!(matches!(
                select_addresses(name, 443, ips.into_iter().map(socket).collect(), never()).await,
                Err(AppError::Denied(_))
            ));
        }
    }

    #[test]
    fn dns_answers_are_bound_to_the_requested_name_type_and_alias_chain() {
        let original = json!({"Status":0,"TC":false,"Question":[{"name":"mod.example.org.","type":1}],"Answer":[
            {"name":"mod.example.org.","type":5,"data":"files.example.net."},
            {"name":"FILES.example.net.","type":1,"data":"1.2.3.4"},
            {"name":"elsewhere.example.net.","type":1,"data":"5.6.7.8"}]});
        let answer: DnsAnswer = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(
            answer.addresses("mod.example.org", 1).unwrap(),
            [socket("1.2.3.4").ip()]
        );
        assert!(answer.addresses("another.example.org", 1).is_err());
        assert!(answer.addresses("mod.example.org", 28).is_err());
        for (pointer, value) in [
            ("/TC", json!(true)),
            ("/Status", json!(3)),
            ("/Answer/0/data", json!("mod.example.org")),
            ("/Answer/1/data", json!("::1")),
        ] {
            let mut response = original.clone();
            *response.pointer_mut(pointer).unwrap() = value;
            let answer: DnsAnswer = serde_json::from_value(response).unwrap();
            assert!(answer.addresses("mod.example.org", 1).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires the public resolver and the Mod Inn HTTPS site; sends no credentials"]
    async fn live_mod_inn_public_dns_and_pinned_https() {
        let name = "www.glossmod.com";
        let ips = public_lookup(name).await.unwrap();
        assert!(!ips.is_empty() && ips.iter().copied().all(public_ip));
        let url = super::super::validate_download_url("https://www.glossmod.com/").unwrap();
        let client = super::super::pinned_download_client(&url, Duration::from_secs(15))
            .await
            .unwrap();
        let response = client.head(url).send().await.unwrap();
        assert!(response.status().is_success());
        assert!(public_ip(response.remote_addr().unwrap().ip()));
    }
}
