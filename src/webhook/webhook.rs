use hyper::{Body, Request, Response, StatusCode, Method};
use serde::Deserialize;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use reqwest::Client;
use dotenv_codegen::dotenv;

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_SECRET: &str = dotenv!("GITHUB_WEBHOOK_SECRET");
const BUILDER_ENDPOINT: &str = "http://localhost:8084/build";

#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
  #[serde(rename = "ref")]
  pub ref_field: Option<String>,
  pub before: Option<String>,
  pub after: Option<String>,
  pub repository: Option<Repository>,
  pub commits: Option<Vec<Commit>>,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: String,
}
#[derive(Debug, Deserialize)]
pub struct Commit {
    pub id: String,
    pub message: String,
    pub url: String,
    pub distinct: bool,
}

async fn handle_webhook(payload: WebhookPayload) {
    if let Some(ref_field) = payload.ref_field {
        println!("Ref: {}", ref_field);
    }

    if let Some(repository) = payload.repository {
        println!("Repository: {}", repository.name);
        println!("Repository URL: {}", repository.url);
    }

    if let Some(commits) = payload.commits {
        for commit in commits {
            println!("Commit: {} - {}", commit.id, commit.message);
        }

        let client = Client::new();
        let _ = client.get(BUILDER_ENDPOINT).send().await;
    }
}

pub async fn handle_request(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
        let signature = req.headers().get("X-Hub-Signature-256").map(|value| value.to_str().unwrap().to_owned());
    
        match (req.method(), req.uri().path()) {
            (&Method::POST, "/webhook") => {
                let whole_body = hyper::body::to_bytes(req.into_body()).await?;
                
                let mut mac = HmacSha256::new_from_slice(WEBHOOK_SECRET.as_bytes()).expect("Invalid HMAC key");
    
                mac.update(&whole_body);
                let result = mac.finalize();
                let code_bytes = result.into_bytes();
    
                if let Some(signature) = signature {
                    let (_, hex_signature) = signature.split_at(7);
                    let signature_bytes = hex::decode(hex_signature).unwrap();
                    if code_bytes.as_slice() != signature_bytes.as_slice() {
                        return Ok(Response::builder()
                            .status(StatusCode::FORBIDDEN)
                            .body(Body::from("Invalid signature"))
                            .unwrap());
                    }
                } else {
                    return Ok(Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .body(Body::from("Invalid signature"))
                        .unwrap());
                }
    
                let payload: WebhookPayload = serde_json::from_slice(&whole_body).unwrap();
                
                if payload.commits.is_some() && payload.ref_field.as_ref().map_or(false, |s| s.starts_with("refs/heads/")) {
                    handle_webhook(payload).await;
                }
    
                Ok(Response::new(Body::from("Webhook receiver")))
    
    
            },
            _ => {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("Not found"))
                    .unwrap())
            }        
        }
}