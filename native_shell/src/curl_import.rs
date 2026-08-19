//! Import a desktop download from a copied cURL command.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CurlDownload {
    pub url: String,
    pub method: String,
    pub body: String,
    pub referer: String,
    pub cookie: String,
    pub user_agent: String,
    pub headers: BTreeMap<String, String>,
}

pub fn parse_curl_command(command: &str) -> Result<Option<CurlDownload>, String> {
    let args = tokenize(command)?;
    if args
        .first()
        .is_none_or(|first| !first.eq_ignore_ascii_case("curl") && !first.eq_ignore_ascii_case("curl.exe"))
    {
        return Ok(None);
    }
    let mut url = String::new();
    let mut method = "GET".to_string();
    let mut body = String::new();
    let mut headers = BTreeMap::new();
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--url" || arg == "-X" || arg == "--request" || arg == "-H" || arg == "--header"
            || arg == "-A" || arg == "--user-agent" || arg == "-e" || arg == "--referer"
            || arg == "-b" || arg == "--cookie" || arg == "-u" || arg == "--user"
            || arg == "-d" || arg == "--data" || arg == "--data-raw" || arg == "--data-binary"
            || arg == "--data-urlencode" || arg == "-o" || arg == "--output" || arg == "--proxy"
        {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("{arg} 缺少参数"))?
                .clone();
            index += 2;
            match arg.as_str() {
                "--url" => url = value,
                "-X" | "--request" => method = value.to_ascii_uppercase(),
                "-H" | "--header" => {
                    if let Some((name, header)) = value.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), header.trim().to_string());
                    }
                }
                "-A" | "--user-agent" => {
                    headers.insert("user-agent".into(), value);
                }
                "-e" | "--referer" => {
                    headers.insert("referer".into(), value);
                }
                "-b" | "--cookie" => {
                    headers.insert("cookie".into(), value);
                }
                "-u" | "--user" => {
                    headers.insert("authorization".into(), basic_auth(&value));
                }
                "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-urlencode" => {
                    if value.starts_with('@') {
                        return Err("不能导入引用本机文件的 cURL 请求体".into());
                    }
                    body = value;
                    if method == "GET" {
                        method = "POST".into();
                    }
                }
                _ => {}
            }
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        if url.is_empty() {
            url = arg.clone();
        }
        index += 1;
    }
    if !crate::http_engine::http_fetch_url_allowed(&url) {
        return Err("cURL 命令中没有有效的 HTTP(S) 地址".into());
    }
    if body.contains('\r') || body.contains('\n') {
        return Err("cURL 请求体不能包含换行".into());
    }
    headers.retain(|key, value| {
        !key.contains(['\r', '\n', ':']) && !value.contains('\r') && !value.contains('\n')
    });
    if !body.is_empty() && !headers.contains_key("content-type") {
        headers.insert(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        );
    }
    let referer = headers.remove("referer").unwrap_or_default();
    let cookie = headers.remove("cookie").unwrap_or_default();
    let user_agent = headers.remove("user-agent").unwrap_or_default();
    headers.remove("origin");
    headers.remove("content-length");
    headers.remove("range");
    Ok(Some(CurlDownload {
        url,
        method,
        body,
        referer,
        cookie,
        user_agent,
        headers,
    }))
}

fn basic_auth(value: &str) -> String {
    format!("Basic {}", base64_encode(value.as_bytes()))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (a << 16) | (b << 8) | c;
        out.push(TABLE[((triple >> 18) & 63) as usize] as char);
        out.push(TABLE[((triple >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn tokenize(command: &str) -> Result<Vec<String>, String> {
    let flattened = command.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut values = Vec::new();
    let mut value = String::new();
    let mut quote = '\0';
    let mut escaped = false;
    for char in flattened.chars() {
        if escaped {
            value.push(char);
            escaped = false;
            continue;
        }
        if char == '\\' && quote != '\'' {
            escaped = true;
            continue;
        }
        if quote != '\0' {
            if char == quote {
                quote = '\0';
            } else {
                value.push(char);
            }
            continue;
        }
        if char == '"' || char == '\'' {
            quote = char;
            continue;
        }
        if char.is_whitespace() {
            if !value.is_empty() {
                values.push(std::mem::take(&mut value));
            }
            continue;
        }
        value.push(char);
    }
    if quote != '\0' {
        return Err("cURL 命令的引号没有闭合".into());
    }
    if !value.is_empty() {
        values.push(value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_post_with_headers() {
        let parsed = parse_curl_command(
            r#"curl -X POST -H "Cookie: a=1" -e https://ref.test -d "q=1" https://cdn.test/file.bin"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.url, "https://cdn.test/file.bin");
        assert_eq!(parsed.cookie, "a=1");
        assert_eq!(parsed.referer, "https://ref.test");
        assert_eq!(parsed.body, "q=1");
    }

    #[test]
    fn rejects_crlf_in_url() {
        assert!(parse_curl_command("curl \"http://cdn.test/x\r\nHost: evil\"").is_err());
    }

    #[test]
    fn ignores_plain_text() {
        assert_eq!(
            parse_curl_command("https://cdn.test/file.bin").unwrap(),
            None
        );
    }
}
