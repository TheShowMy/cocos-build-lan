use gloo_net::http::{Request, Response};
use serde::{Serialize, de::DeserializeOwned};

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    decode(Request::get(path).send().await.map_err(net_error)?).await
}

pub async fn get_text(path: &str) -> Result<String, String> {
    let response = Request::get(path).send().await.map_err(net_error)?;
    decode_text(response).await
}

pub async fn post<T: DeserializeOwned, B: Serialize>(path: &str, body: &B) -> Result<T, String> {
    let request = Request::post(path).json(body).map_err(net_error)?;
    decode(request.send().await.map_err(net_error)?).await
}

pub async fn post_empty<B: Serialize>(path: &str, body: &B) -> Result<(), String> {
    let request = Request::post(path).json(body).map_err(net_error)?;
    decode_empty(request.send().await.map_err(net_error)?).await
}

pub async fn put<T: DeserializeOwned, B: Serialize>(path: &str, body: &B) -> Result<T, String> {
    let request = Request::put(path).json(body).map_err(net_error)?;
    decode(request.send().await.map_err(net_error)?).await
}

pub async fn delete(path: &str) -> Result<(), String> {
    decode_empty(Request::delete(path).send().await.map_err(net_error)?).await
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T, String> {
    let text = decode_text(response).await?;
    serde_json::from_str(&text).map_err(|error| format!("响应数据格式无效: {error}"))
}

async fn decode_empty(response: Response) -> Result<(), String> {
    decode_text(response).await.map(|_| ())
}

async fn decode_text(response: Response) -> Result<String, String> {
    let status = response.status();
    let ok = response.ok();
    let text = response.text().await.map_err(net_error)?;
    if ok {
        Ok(text)
    } else if text.trim().is_empty() {
        Err(format!("请求失败（HTTP {status}）"))
    } else {
        Err(text)
    }
}

fn net_error(error: impl std::fmt::Display) -> String {
    format!("无法连接服务: {error}")
}

pub fn encode_query(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
