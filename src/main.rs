//! gray-searchapi — keyed web search across Brave, SerpApi, and Linkup.
//!
//! Three tools — `brave_web_search` (BRAVE_API_KEY), `serpapi_search`
//! (SERPAPI_KEY), `linkup_search` (LINKUP_API_KEY) — each answering
//! `{"results":[{title,url,snippet}]}`. Keys come from the environment or
//! `~/.gray/searchapi/config.json` (written by `searchapi set --brave|…`);
//! env wins. `/searchapi status` reports which providers are configured.
//! All network I/O shells out to curl.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

const DEFAULT_COUNT: usize = 10;

fn manifest() -> Value {
    json!({
        "name": "searchapi",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.3",
        "tools": [
            {
                "name": "brave_web_search",
                "description": "Search the web via the Brave Search API. Needs BRAVE_API_KEY (env, or `searchapi set --brave KEY`). Returns {results:[{title,url,snippet}]}.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query."},
                        "count": {"type": "integer", "description": "Max results, 1-20 (default 10)."}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "serpapi_search",
                "description": "Search the web via SerpApi. Needs SERPAPI_KEY. Returns {results:[{title,url,snippet}]}.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query."},
                        "engine": {"type": "string", "description": "SerpApi engine id (default google)."},
                        "count": {"type": "integer", "description": "Max results, 1-100 (default 10)."}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "linkup_search",
                "description": "Search the web via the Linkup API. Needs LINKUP_API_KEY. depth 'standard' (default) or 'deep'. Returns {results:[{title,url,snippet}]}.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query."},
                        "depth": {"type": "string", "enum": ["standard", "deep"],
                                  "description": "standard (default) is fast; deep is slower and more thorough."},
                        "count": {"type": "integer", "description": "Max results (default 10)."}
                    },
                    "required": ["query"]
                }
            }
        ],
        "commands": ["/searchapi"],
    })
}

// ---------- config -----------------------------------------------------------

fn state_dir() -> PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("searchapi")
}

fn read_config() -> Value {
    std::fs::read_to_string(state_dir().join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

// (tool name, env var, config.json key)
fn providers() -> [(&'static str, &'static str, &'static str); 3] {
    [
        ("brave_web_search", "BRAVE_API_KEY", "brave_api_key"),
        ("serpapi_search", "SERPAPI_KEY", "serpapi_key"),
        ("linkup_search", "LINKUP_API_KEY", "linkup_api_key"),
    ]
}

fn api_key(env: &str, cfg_key: &str) -> Option<String> {
    std::env::var(env)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| read_config()[cfg_key].as_str().map(str::to_string))
}

fn key_source(env: &str, cfg_key: &str) -> Option<&'static str> {
    if std::env::var(env).map(|v| !v.trim().is_empty()).unwrap_or(false) {
        Some("env")
    } else if read_config()[cfg_key].as_str().is_some_and(|s| !s.is_empty()) {
        Some("config")
    } else {
        None
    }
}

// ---------- http (curl) ------------------------------------------------------

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// curl with `-w \n%{http_code}`; returns (status, body).
fn run_curl(extra: &[String]) -> Result<(u16, String), String> {
    let mut args: Vec<String> = vec![
        "-sS".into(),
        "--max-time".into(),
        "15".into(),
        "-w".into(),
        "\n%{http_code}".into(),
    ];
    args.extend(extra.iter().cloned());
    let out = std::process::Command::new("curl")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, status) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    let status: u16 = status.trim().parse().unwrap_or(0);
    if status == 0 && !out.status.success() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok((status, body.to_string()))
}

fn api_err(provider: &str, status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            ["/error/message", "/error", "/message", "/detail"]
                .iter()
                .filter_map(|p| v.pointer(p))
                .map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string()))
                .next()
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    format!("{provider} → HTTP {status}: {}", detail.chars().take(300).collect::<String>())
}

// ---------- providers --------------------------------------------------------

struct Hit {
    title: String,
    url: String,
    snippet: String,
}

fn collect(items: Option<&Vec<Value>>, t: &str, u: &str, s: &str, limit: usize) -> Vec<Hit> {
    items
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(Hit {
                        title: r.get(t)?.as_str()?.to_string(),
                        url: r.get(u)?.as_str()?.to_string(),
                        snippet: r.get(s).and_then(Value::as_str).unwrap_or("").to_string(),
                    })
                })
                .take(limit)
                .collect()
        })
        .unwrap_or_default()
}

fn brave(query: &str, count: usize, token: &str) -> Result<Vec<Hit>, String> {
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={count}",
        url_encode(query)
    );
    let auth = format!("X-Subscription-Token: {token}");
    let (status, body) = run_curl(&["-H".into(), auth, "-H".into(), "Accept: application/json".into(), url])?;
    if !(200..300).contains(&status) {
        return Err(api_err("brave", status, &body));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("brave: unreadable reply: {e}"))?;
    Ok(collect(v.pointer("/web/results").and_then(Value::as_array), "title", "url", "description", count))
}

fn serpapi(query: &str, engine: &str, count: usize, token: &str) -> Result<Vec<Hit>, String> {
    let url = format!(
        "https://serpapi.com/search.json?engine={}&q={}&num={count}&api_key={}",
        url_encode(engine),
        url_encode(query),
        url_encode(token)
    );
    let (status, body) = run_curl(&[url])?;
    if !(200..300).contains(&status) {
        return Err(api_err("serpapi", status, &body));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("serpapi: unreadable reply: {e}"))?;
    Ok(collect(v.get("organic_results").and_then(Value::as_array), "title", "link", "snippet", count))
}

fn linkup(query: &str, depth: &str, count: usize, token: &str) -> Result<Vec<Hit>, String> {
    let req = json!({"q": query, "depth": depth, "outputType": "searchResults"});
    let auth = format!("Authorization: Bearer {token}");
    let (status, body) = run_curl(&[
        "-X".into(), "POST".into(),
        "-H".into(), auth,
        "-H".into(), "Content-Type: application/json".into(),
        "-d".into(), req.to_string(),
        "https://api.linkup.so/v1/search".into(),
    ])?;
    if !(200..300).contains(&status) {
        return Err(api_err("linkup", status, &body));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("linkup: unreadable reply: {e}"))?;
    // searchResults → results[]; sourcedAnswer-style replies carry sources[].
    let hits = match v.get("results").and_then(Value::as_array) {
        Some(items) => collect(Some(items), "name", "url", "content", count),
        None => collect(v.get("sources").and_then(Value::as_array), "name", "url", "snippet", count),
    };
    Ok(hits)
}

fn hits_json(hits: &[Hit]) -> String {
    let results: Vec<Value> = hits
        .iter()
        .map(|h| json!({"title": h.title, "url": h.url, "snippet": h.snippet}))
        .collect();
    json!({"results": results}).to_string()
}

// ---------- tool + command ---------------------------------------------------

fn call_tool(name: &str, args: &Value) -> Value {
    let Some((_, env, cfg_key)) = providers().into_iter().find(|(n, _, _)| *n == name) else {
        return json!({"content": format!("unknown tool: {name}"), "is_error": true});
    };
    let query = args.get("query").and_then(Value::as_str).unwrap_or("").trim();
    if query.is_empty() {
        return json!({"content": format!("{name} needs a non-empty query"), "is_error": true});
    }
    let Some(token) = api_key(env, cfg_key) else {
        return json!({
            "content": format!("{env} is not set — export it, or `searchapi set` a key into ~/.gray/searchapi/config.json"),
            "is_error": true,
        });
    };
    let count = args
        .get("count")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_COUNT);
    let hits = match name {
        "brave_web_search" => brave(query, count.clamp(1, 20), &token),
        "serpapi_search" => serpapi(
            query,
            args.get("engine").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("google"),
            count.clamp(1, 100),
            &token,
        ),
        "linkup_search" => match args.get("depth").and_then(Value::as_str).unwrap_or("standard") {
            d @ ("standard" | "deep") => linkup(query, d, count.clamp(1, 50), &token),
            other => Err(format!("bad depth '{other}' — standard|deep")),
        },
        _ => unreachable!(),
    };
    match hits {
        Ok(h) => json!({"content": hits_json(&h)}),
        Err(e) => json!({"content": e, "is_error": true}),
    }
}

fn status_text() -> String {
    let mut out = String::from("searchapi — keyed web search\n");
    for (tool, env, cfg_key) in providers() {
        let st = match key_source(env, cfg_key) {
            Some(src) => format!("set ({src})"),
            None => "MISSING".into(),
        };
        out.push_str(&format!("  {tool:<17} {env:<15} {st}\n"));
    }
    out.push_str("keys: env wins, else ~/.gray/searchapi/config.json (`searchapi set`)");
    out
}

fn cli(argv: &[String]) -> Result<String, String> {
    match argv.first().map(String::as_str) {
        Some("set") => {
            let mut c = read_config();
            let mut it = argv[1..].iter();
            let mut n = 0;
            while let Some(f) = it.next() {
                let key = match f.as_str() {
                    "--brave" => "brave_api_key",
                    "--serpapi" => "serpapi_key",
                    "--linkup" => "linkup_api_key",
                    other => return Err(format!("unknown flag {other} — --brave|--serpapi|--linkup")),
                };
                c[key] = json!(it.next().ok_or_else(|| format!("{f} needs a key value"))?);
                n += 1;
            }
            if n == 0 {
                return Err("set needs at least one of --brave|--serpapi|--linkup KEY".into());
            }
            let dir = state_dir();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join("config.json");
            std::fs::write(&path, serde_json::to_string_pretty(&c).unwrap()).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(format!("saved → {}", path.display()))
        }
        Some("status") | None => Ok(status_text()),
        Some("-h" | "--help" | "help") => Ok(
            "usage: searchapi set [--brave|--serpapi|--linkup KEY] | status".into()),
        Some(o) => Err(format!("unknown verb {o} — status|set")),
    }
}

// ---------- wire -------------------------------------------------------------

fn handle(req: &Value) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "plugin/tools" => json!({ "tools": manifest()["tools"] }),
        "tool/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("args").cloned().unwrap_or(Value::Null);
            call_tool(name, &args)
        }
        "command/run" => {
            let argv: Vec<String> = params
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            json!({ "text": cli(&argv).unwrap_or_else(|e| e) })
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn sidecar() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let (reply, exit) = handle(&req);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<String> = match args.first().map(String::as_str) {
        Some("searchapi") | Some("gray-searchapi") => args[1..].to_vec(),
        _ => args,
    };
    match args.first().map(String::as_str) {
        Some("manifest") => {
            println!("{}", manifest());
            Ok(())
        }
        Some("sidecar") => sidecar(),
        _ if args.is_empty() => {
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                println!("{}", status_text());
                println!("keys: `searchapi set --brave K --serpapi K --linkup K`");
                Ok(())
            } else {
                sidecar()
            }
        }
        _ => match cli(&args) {
            Ok(t) => {
                println!("{t}");
                Ok(())
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params })).0.unwrap()
    }

    #[test]
    fn manifest_shape() {
        let m = call("plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "searchapi");
        let names: Vec<&str> = m["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["brave_web_search", "serpapi_search", "linkup_search"]);
        assert_eq!(m["commands"], json!(["/searchapi"]));
        assert!(m["tools"].as_array().unwrap().iter().all(|t| t["parameters"]["required"] == json!(["query"])));
    }

    #[test]
    fn plugin_tools_lists_all() {
        let r = call("plugin/tools", json!({}));
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn unkeyed_and_bad_args_are_tool_errors() {
        unsafe {
            std::env::set_var("GRAY_HOME", std::env::temp_dir().join("searchapi-test-nokeys"));
            for (_, env, _) in providers() {
                std::env::remove_var(env);
            }
        }
        let r = call("tool/call", json!({ "name": "brave_web_search", "args": {"query": "x"} }));
        assert_eq!(r["result"]["is_error"], true);
        assert!(r["result"]["content"].as_str().unwrap().contains("BRAVE_API_KEY"));
        let r = call("tool/call", json!({ "name": "linkup_search", "args": {"query": " "} }));
        assert!(r["result"]["content"].as_str().unwrap().contains("query"));
        let r = call("tool/call", json!({ "name": "nope", "args": {} }));
        assert!(r["result"]["content"].as_str().unwrap().contains("unknown tool"));
    }

    #[test]
    fn collect_maps_provider_fields() {
        let brave = json!([{"title": "T", "url": "https://a.b", "description": "s"},
                           {"title": "T2", "url": "https://c.d"}]);
        let h = collect(brave.as_array(), "title", "url", "description", 10);
        assert_eq!((h[0].title.as_str(), h[0].snippet.as_str()), ("T", "s"));
        assert_eq!(h[1].snippet, "");
        assert_eq!(
            hits_json(&h),
            r#"{"results":[{"title":"T","url":"https://a.b","snippet":"s"},{"title":"T2","url":"https://c.d","snippet":""}]}"#
        );
        let serp = json!([{"title": "G", "link": "https://g", "snippet": "gs"}]);
        assert_eq!(collect(serp.as_array(), "title", "link", "snippet", 10)[0].url, "https://g");
        let link = json!([{"type": "text", "name": "L", "url": "https://l", "content": "lc"}]);
        assert_eq!(collect(link.as_array(), "name", "url", "content", 10)[0].title, "L");
    }

    #[test]
    fn status_lists_each_provider() {
        unsafe { std::env::set_var("GRAY_HOME", "/tmp/searchapi-test-nokeys") };
        let t = status_text();
        for s in ["brave_web_search", "serpapi_search", "linkup_search", "BRAVE_API_KEY", "MISSING"] {
            assert!(t.contains(s), "status missing {s}");
        }
        let r = call("command/run", json!({ "name": "/searchapi", "argv": ["status"] }));
        assert!(r["result"]["text"].as_str().unwrap().contains("linkup"));
        let r = call("command/run", json!({ "name": "/searchapi", "argv": ["bogus-verb"] }));
        assert!(r["result"]["text"].as_str().unwrap().contains("unknown verb"));
    }

    #[test]
    fn url_encode_and_shutdown() {
        assert_eq!(url_encode("a b+c&d"), "a%20b%2Bc%26d");
        let (reply, exit) = handle(&json!({ "id": 2, "method": "plugin/shutdown" }));
        assert!(reply.is_some() && exit);
        assert_eq!(call("nope", Value::Null)["error"]["code"], -32601);
    }
}
