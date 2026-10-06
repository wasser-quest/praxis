//! Backend-authoritative remote TUI operations. No key in URLs or local storage.
use anyhow::Context;
use serde_json::{json,Value};
#[derive(Clone)]
pub struct Remote {
    url:String,
    key:String,
    client:reqwest::Client,
}
impl Remote {
    pub fn new(url:String,key:String) -> anyhow::Result<Self> {
        let parsed=reqwest::Url::parse(&url)?;
        anyhow::ensure!(matches!(parsed.scheme(),"http"|"https") && parsed.username().is_empty() && parsed.password().is_none() && parsed.query().is_none(),"Use an HTTP(S) gateway URL without embedded credentials or query parameters");
        anyhow::ensure!(!key.trim().is_empty(),"Set --gateway-key or PRAXIS_GATEWAY_KEY");
        Ok(Self {url:url.trim_end_matches('/').into(),key,client:reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?})
    }
    pub async fn request(&self,method:reqwest::Method,path:&str,body:Option<Value>) -> anyhow::Result<Value> {
        let mut r=self.client.request(method,format!("{}{path}",self.url)).bearer_auth(&self.key);
        if let Some(body)=body {r=r.json(&body);}
        let r=r.send().await.map_err(|_|anyhow::anyhow!("Gateway connection failed"))?;
        anyhow::ensure!(r.status().is_success(),"Gateway returned HTTP {} (check gateway URL, key and backend version)",r.status().as_u16());
        r.json().await.context("Invalid gateway JSON response")
    }
    pub async fn command(&self,user:&str,line:&str) -> anyhow::Result<String> {
        let data=self.request(reqwest::Method::POST,"/v1/context/exec",Some(json!({"user_id":user,"line":line}))).await?;
        let result=data["result"].as_str().context("No command result")?;
        anyhow::ensure!(!result.starts_with('✗') && !result.starts_with("load failed:"),"Backend context command failed");
        Ok(result.into())
    }
    pub async fn messages(&self,user:&str) -> anyhow::Result<Vec<crate::model::Message>> {
        let value=self.request(reqwest::Method::GET,&format!("/v1/messages/{}",urlencoding::encode(user)),None).await?;
        Ok(serde_json::from_value(value["messages"].clone())?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_client_rejects_embedded_credentials_and_empty_key() {
        assert!(Remote::new("http://user:secret@host".into(),"test".into()).is_err());
        assert!(Remote::new("http://host?token=secret".into(),"test".into()).is_err());
        assert!(Remote::new("file:///tmp/test".into(),"test".into()).is_err());
        assert!(Remote::new("http://host".into(),"".into()).is_err());
    }
    #[tokio::test]
    async fn remote_client_uses_backend_history_and_bearer_header() {
        use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{method,path,header}};
        let s=MockServer::start().await;
        Mock::given(method("GET")).and(path("/v1/messages/synthetic")).and(header("Authorization","Bearer synthetic-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messages":[{"role":"assistant","content":"remote fixture","id":42}]})))
            .expect(1).mount(&s).await;
        let remote=Remote::new(s.uri(),"synthetic-key".into()).unwrap();
        assert_eq!(remote.messages("synthetic").await.unwrap()[0].content,"remote fixture");
    }
}
