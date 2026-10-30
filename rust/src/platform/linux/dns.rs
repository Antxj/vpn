//! Linux: the VPN's DNS servers and domains, as the server pushes them.
//!
//! Windows applies them to the VPN adapter by itself; on Linux OpenVPN only
//! hands them to a script. Without this, a VPN that uses split-horizon DNS
//! (an internal name resolving to an internal address only through the VPN's
//! DNS) gets the public address instead and the site refuses the access.
//!
//! The helper starts openvpn with `--up "<HELPER> --vpn-dns up"` and
//! `--down "<HELPER> --vpn-dns down"`. OpenVPN (2.5, 2.6 and 2.7 alike) then
//! runs this hook as root with the tunnel device as first argument and the
//! pushed options in `foreign_option_<n>` ("dhcp-option DNS 10.0.0.45",
//! "dhcp-option DOMAIN example.org"). It applies them to the tunnel through
//! systemd-resolved (Ubuntu, Mint, Fedora...) or resolvconf, and prints one
//! `vpn-dns:` line that the app shows in its log (vpn.rs).

use std::io::Write;
use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};

/// Command line argument of the hook (followed by "up" or "down").
pub const ARG_DNS_HOOK: &str = "--vpn-dns";

/// Prefix of the line the app looks for in the OpenVPN log.
use crate::vpn::DNS_LOG_PREFIX as LOG_PREFIX;

const MAX_SERVERS: usize = 8;
const MAX_DOMAINS: usize = 16;

/// DNS settings pushed by the server, already validated.
#[derive(Debug, Default, PartialEq)]
pub struct PushedDns {
    pub servers: Vec<IpAddr>,
    pub domains: Vec<String>,
}

/// A domain name made of letters, digits and hyphens (labels up to 63
/// characters, 253 in all): safe to hand to a command run as root.
fn valid_domain(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Tunnel device name as OpenVPN gives it (tun0, tap1...).
fn valid_device(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Reads the pushed DNS options from the `foreign_option_<n>` variables.
/// Anything that is not a valid address or domain is ignored.
pub fn from_foreign_options(vars: impl Iterator<Item = (String, String)>) -> PushedDns {
    let mut options: Vec<(u32, String)> = vars
        .filter_map(|(name, value)| {
            let n = name.strip_prefix("foreign_option_")?.parse().ok()?;
            Some((n, value))
        })
        .collect();
    options.sort();
    let mut dns = PushedDns::default();
    for (_, option) in options {
        let parts: Vec<&str> = option.split_whitespace().collect();
        match parts.as_slice() {
            ["dhcp-option", "DNS" | "DNS6", address] => {
                if let Ok(ip) = address.parse::<IpAddr>() {
                    if !dns.servers.contains(&ip) && dns.servers.len() < MAX_SERVERS {
                        dns.servers.push(ip);
                    }
                }
            }
            ["dhcp-option", "DOMAIN" | "DOMAIN-SEARCH" | "ADAPTER-DOMAIN-SUFFIX", domain] => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                if valid_domain(&domain)
                    && !dns.domains.contains(&domain)
                    && dns.domains.len() < MAX_DOMAINS
                {
                    dns.domains.push(domain);
                }
            }
            _ => {}
        }
    }
    dns
}

/// A system tool, used only when root owns it (and nobody else can change it).
fn tool(candidates: &[&'static str]) -> Option<&'static str> {
    candidates
        .iter()
        .copied()
        .find(|p| crate::platform::unix::helper::do_root_e_protegido(p))
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// systemd-resolved is running (its D-Bus socket exists) and resolvectl is there.
fn resolvectl() -> Option<&'static str> {
    let running = Path::new("/run/systemd/resolve/io.systemd.Resolve").exists()
        || Path::new("/run/systemd/resolve/stub-resolv.conf").exists();
    if !running {
        return None;
    }
    tool(&["/usr/bin/resolvectl", "/bin/resolvectl"])
}

fn resolvconf() -> Option<&'static str> {
    tool(&["/usr/sbin/resolvconf", "/sbin/resolvconf"])
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "-".into()
    } else {
        items.join(",")
    }
}

/// Applies the settings to the tunnel. Returns the `vpn-dns:` line.
fn apply(device: &str, dns: &PushedDns) -> String {
    let servers: Vec<String> = dns.servers.iter().map(IpAddr::to_string).collect();
    if servers.is_empty() {
        return format!("{LOG_PREFIX} none");
    }
    let summary = format!("servers={} domains={}", list(&servers), list(&dns.domains));

    // systemd-resolved: the servers belong to the tunnel; the domains are
    // search and routing domains, so their names are asked to the VPN's DNS
    if let Some(resolvectl) = resolvectl() {
        let mut dns_args = vec!["dns", device];
        dns_args.extend(servers.iter().map(String::as_str));
        let mut domain_args = vec!["domain", device];
        domain_args.extend(dns.domains.iter().map(String::as_str));
        let result = run(resolvectl, &dns_args)
            .and_then(|_| {
                if dns.domains.is_empty() {
                    Ok(())
                } else {
                    run(resolvectl, &domain_args)
                }
            })
            // answers cached before the VPN (the public address) go away
            .and_then(|_| run(resolvectl, &["flush-caches"]));
        return match result {
            Ok(()) => format!("{LOG_PREFIX} applied resolver=systemd-resolved {summary}"),
            Err(e) => format!("{LOG_PREFIX} failed resolver=systemd-resolved {summary} error={e}"),
        };
    }

    // resolvconf: one record for the tunnel, removed by "down"
    if let Some(resolvconf) = resolvconf() {
        let mut text = String::new();
        for s in &servers {
            text.push_str(&format!("nameserver {s}\n"));
        }
        if !dns.domains.is_empty() {
            text.push_str(&format!("search {}\n", dns.domains.join(" ")));
        }
        let record = format!("{device}.vpn");
        let result = Command::new(resolvconf)
            .args(["-a", &record])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait_with_output()
            });
        return match result {
            Ok(o) if o.status.success() => {
                format!("{LOG_PREFIX} applied resolver=resolvconf {summary}")
            }
            Ok(o) => format!(
                "{LOG_PREFIX} failed resolver=resolvconf {summary} error={}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => format!("{LOG_PREFIX} failed resolver=resolvconf {summary} error={e}"),
        };
    }

    format!("{LOG_PREFIX} not-applied {summary} error=no systemd-resolved or resolvconf")
}

/// Undoes `apply` (the tunnel device may already be gone).
fn revert(device: &str) {
    if let Some(resolvectl) = resolvectl() {
        let _ = run(resolvectl, &["revert", device]);
        let _ = run(resolvectl, &["flush-caches"]);
    }
    if let Some(resolvconf) = resolvconf() {
        let _ = run(resolvconf, &["-d", &format!("{device}.vpn"), "-f"]);
    }
}

/// The hook itself: `<HELPER> --vpn-dns up|down <device> ...` run by
/// openvpn as root. Returns the exit code (never fails the connection).
pub fn hook(args: &[String]) -> i32 {
    // only openvpn, started by the helper, runs this: never a user through pkexec
    if unsafe { libc::geteuid() } != 0 || std::env::var_os("PKEXEC_UID").is_some() {
        eprintln!("{LOG_PREFIX} refused (not run by openvpn)");
        return 1;
    }
    let (Some(phase), Some(device)) = (args.first(), args.get(1)) else {
        return 1;
    };
    if !valid_device(device) {
        eprintln!("{LOG_PREFIX} refused (device name)");
        return 1;
    }
    match phase.as_str() {
        "up" => {
            let dns = from_foreign_options(std::env::vars());
            println!("{}", apply(device, &dns));
        }
        "down" => revert(device),
        _ => return 1,
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(items: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn reads_the_pushed_dns() {
        let dns = from_foreign_options(vars(&[
            ("foreign_option_2", "dhcp-option DNS 10.0.0.46"),
            ("foreign_option_1", "dhcp-option DNS 10.0.0.45"),
            ("foreign_option_3", "dhcp-option DOMAIN Example.org."),
            ("foreign_option_4", "dhcp-option DNS6 fd00::53"),
            (
                "foreign_option_5",
                "dhcp-option DOMAIN-SEARCH corp.example.org",
            ),
            ("foreign_option_6", "dhcp-option WINS 10.0.0.9"),
            ("dev", "tun0"),
        ]));
        assert_eq!(
            dns.servers,
            ["10.0.0.45", "10.0.0.46", "fd00::53"]
                .map(|s| s.parse::<IpAddr>().unwrap())
                .to_vec()
        );
        assert_eq!(dns.domains, ["example.org", "corp.example.org"]);
    }

    #[test]
    fn refuses_what_is_not_an_address_or_a_domain() {
        let dns = from_foreign_options(vars(&[
            ("foreign_option_1", "dhcp-option DNS 10.0.0.1;reboot"),
            ("foreign_option_2", "dhcp-option DNS $(id)"),
            ("foreign_option_3", "dhcp-option DOMAIN a b"),
            ("foreign_option_4", "dhcp-option DOMAIN -x.org"),
            ("foreign_option_5", "dhcp-option DOMAIN ex_ample.org"),
            ("foreign_option_6", "dhcp-option DOMAIN ../etc"),
        ]));
        assert_eq!(dns, PushedDns::default());
        assert!(valid_device("tun0") && valid_device("tap-vpn"));
        assert!(!valid_device("tun0;id") && !valid_device("-x") && !valid_device(""));
        assert!(!valid_domain(&"a".repeat(64)));
    }

    #[test]
    fn nothing_pushed_nothing_applied() {
        assert_eq!(apply("tun0", &PushedDns::default()), "vpn-dns: none");
    }
}
