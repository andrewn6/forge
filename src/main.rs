pub mod logs;
pub mod webhook;

use hyper::body::to_bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, StatusCode, Method, Error};
use hyper::Server;
use reqwest::{Client, Url};

use webhook::webhook::handle_request as handle_webhook;

use nixpacks::nixpacks::builder::docker::DockerBuilderOptions as NixpacksOptions;
use nixpacks::nixpacks::plan::generator::GeneratePlanOptions;
use nixpacks::{create_docker_image, generate_build_plan};

use logs::logs::get_logs;
use logs::logs::LogFilter;
use dotenv::dotenv;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use git2::Repository;
use tempfile::tempdir;
use colored::*;
use std::sync::Arc;
use chrono::{Utc, DateTime};
use tokio::sync::broadcast;

extern crate chrono;
extern crate chrono_tz;
#[derive(Deserialize)]
struct BuildInfo {
	pub path: String,
	pub name: String,
	pub envs: Option<Vec<String>>,
	pub build_options: DockerBuilderOptions,
}

#[derive(Deserialize)]
struct LogParams {
	pub container_id: String,
	pub start_time: DateTime<Utc>,
	pub end_time: DateTime<Utc>,
}

#[derive(Deserialize, Clone, Default, Debug)]
pub struct DockerBuilderOptions {
    pub name: Option<String>,
    pub out_dir: Option<String>,
    pub print_dockerfile: bool,
    pub tags: Vec<String>,
    pub labels: Vec<String>,
    pub quiet: bool,
    pub cache_key: Option<String>,
    pub no_cache: bool,
    pub inline_cache: bool,
    pub cache_from: Option<String>,
    pub platform: Vec<String>,
    pub current_dir: bool,
    pub no_error_without_start: bool,
    pub incremental_cache_image: Option<String>,
    pub verbose: bool,
}

fn convert_to_nixpacks_options(local_options: &DockerBuilderOptions) -> NixpacksOptions {
	NixpacksOptions {
        name: local_options.name.clone(),
        out_dir: local_options.out_dir.clone(),
        print_dockerfile: local_options.print_dockerfile,
        tags: local_options.tags.clone(),
        labels: local_options.labels.clone(),
        quiet: local_options.quiet,
        cache_key: local_options.cache_key.clone(),
        no_cache: local_options.no_cache,
        inline_cache: local_options.inline_cache,
        cache_from: local_options.cache_from.clone(),
        platform: local_options.platform.clone(),
        current_dir: local_options.current_dir,
        no_error_without_start: local_options.no_error_without_start,
        incremental_cache_image: local_options.incremental_cache_image.clone(),
        verbose: local_options.verbose,
    }
}

async fn handle(req: Request<Body>, db_pool: Arc<PgPool>) -> Result<Response<Body>, Error> {
	match (req.method(), req.uri().path()) {

		(&Method::GET, "/") => {
			let html = r#"<!DOCTYPE html>
			<html>
			<style>
			pre {
    			background-color: #f5f5f5;
    			padding: 3px;
			}
			</style>
			<body>
			<h1>nixbuilder</h1>

			<h2>API</h2>
			<p>/build</p>
			<pre><code>curl -X POST -H "Content-Type: application/json" -d '{
				"path": "https://github.com/username/repo.git",
				"name": "image-name",
				"build_options": {
				  "print_dockerfile": false,
				  "tags": ["v1.0", "latest"],
				  "labels": [],
				  "quiet": false,
				  "no_cache": false,
				  "inline_cache": false,
				  "platform": ["linux/amd64"],
				  "current_dir": false,
				  "no_error_without_start": false,
				  "verbose": false
				}
			  }' http://localhost:8084/build</code></pre>
			  
			  <p>/logs</p>
			  <pre><code>curl -X GET \
			  "http://localhost:8084/logs?container_id=<container_id>&start_time=<start_time>&end_time=<end_time>"</code></pre>
			</body>
			</html>"#;

			let response = Response::builder()
				.status(StatusCode::OK)
				.header("Content-Type", "text/html")
				.body(Body::from(html))
				.unwrap();

			Ok(response)
		},
		(&Method::POST, "/webhook") => {
			handle_webhook(req).await
		}

		(&Method::POST, "/build") => {				
			let whole_body = to_bytes(req.into_body()).await?;

			let repo_dir;

			let build_info: BuildInfo = match serde_json::from_slice(&whole_body) {
				Ok(info) => info,
				Err(_) => {
				let response = Response::builder()
					.status(StatusCode::BAD_REQUEST)
					.body(Body::from("Invalid request body"))
					.unwrap();
				return Ok(response);
				}
			};

			if std::path::Path::new(&build_info.path).is_dir() {
				repo_dir = build_info.path.clone();
			} else {
				let temp_dir = tempdir().expect("Failed to create temp dir");
				repo_dir = temp_dir.path().	display().to_string();
				match Repository::clone(&build_info.path, &repo_dir) {
					Ok(_) => eprintln!("Cloned repo successfully"),
					Err(e) => {
						let response = Response::builder()
							.status(StatusCode::BAD_REQUEST)
							.body(Body::from(format!("Failed to clone repository: {}", e)))
							.unwrap();
						return Ok(response);
					}
				}
			}

			if build_info.path.is_empty() || build_info.name.is_empty() {
				let response = Response::builder()
					.status(StatusCode::BAD_REQUEST)
					.body(Body::from("Missing required fields"))
					.unwrap();
				return Ok(response)
			}

			let mut conn = db_pool.acquire().await.unwrap();
			let plan_options = GeneratePlanOptions::default(); // Generate default options
			
			
			let envs: Vec<&str> = if let Some(inner_vec) = &build_info.envs {
				inner_vec.iter().map(|inner_str| inner_str.as_ref()).collect()
			} else {
				Vec::new()
			};

			let plan = generate_build_plan(
				&build_info.path,
				envs,
				&plan_options
			);

			let nixpack_options = convert_to_nixpacks_options(&build_info.build_options);

			let start_time = Utc::now().to_rfc3339();
			let build_if = format!("{}:{}", &build_info.path, &start_time);

			/* Insert build data once build is triggered */
			match sqlx::query("INSERT into build_data (id, start_time, status) VALUES ($1, $2, $3)")
				.bind(&build_if)
				.bind(&start_time)
				.bind("running")
				.execute(&mut conn)
				.await {
				Ok(_) => eprintln!("DB insert success"),
				Err(e) => eprintln!("DB insert error: {}", e), // Or handle the error more properly
			}
			
			let envs: Vec<&str> = if let Some(inner_vec) = &build_info.envs {
				inner_vec.iter().map(|inner_str| inner_str.as_ref()).collect()
			} else {
				Vec::new()
			};

			let result = create_docker_image(
				&repo_dir,
				envs,
				&plan_options,
				&nixpack_options,
			).await;

            /* need to port  registry server from old repo(: 
			let status = match result {
				Ok(_) => {
					let client = Client::new();
					let registry_post_data = json!({
						"image_name": build_info.name,
						"image_tag": build_info.build_options.tags.get(0).unwrap_or(&"latest".to_string())
					});

					let push_result = client.post("http://localhost:8083/push")
						.json(&registry_post_data)
						.send()
						.await;

					match push_result {
						Ok(_) => "Completed",
						Err(_) => "Failed"
					}
				},
				Err(_) => "Failed"
			};
            */

			let end_time = Utc::now().to_rfc3339();
			
			match sqlx::query("UPDATE build_data SET status = $1, end_time = $2 WHERE id = $3")
				.bind(status)
				.bind(&end_time)
				.bind(&build_if)
				.execute(&mut conn)
				.await {
				Ok(_) => eprintln!("DB updated"),
				Err(e) => eprintln!("DB update error: {}", e), // Or handle the error more properly
			}

			let _ = match result {
				Ok(_) => Ok(Response::new(Body::from("Image created."))),
				Err(e) => Err({
					let mut response = Response::new(Body::from(format!("Failed to create image: {}", e)));
					*response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
					response
				})
			};

			Ok(Response::new(Body::from("Image created.")))
		},
		(&Method::GET, "/logs") => {
			let url = Url::parse(&("http://localhost".to_string() + req.uri().path_and_query().map(|x| x.as_str()).unwrap_or(""))).unwrap();

			let params: LogParams = match serde_urlencoded::from_str(url.query().unwrap_or("")) {
				Ok(params) => params,
				Err(_) => {
					return Ok(Response::builder()
					.status(StatusCode::BAD_REQUEST)
					.body(Body::from("Invalid request paramaters"))
					.unwrap());
				}
			};

			let (tx, _) = broadcast::channel(100);
			let filter = LogFilter { start_time: params.start_time, end_time: params.end_time };

			tokio::spawn(async move {
				if let Err(e) = get_logs(&params.container_id, filter, tx).await {
					format!("Error getting logs: {}", e);
				}
			});
			
			Ok(Response::new(Body::from("Logs are being collected.")))

		}
		
		_ => {
			let response = Response::builder()
				.status(StatusCode::NOT_FOUND)
				.body(Body::from("Not found"))
				.unwrap();
			Ok(response)
		}
	}
}

#[tokio::main]
async fn main() {	
	dotenv().ok();

	let db_url = std::env::var("COCKROACH_DB_URL")
		.expect("COCKROACH_DB_URL must be set");

	let db_pool = Arc::new(
		PgPoolOptions::new()
			.max_connections(5)
			.connect(&db_url)
			.await
			.expect("Failed to connect to DB")
	);

	let addr = ([0, 0, 0 ,0], 8084).into();
	
	let make_svc = make_service_fn(move |_conn| {
		let db_pool = Arc::clone(&db_pool);
		async move {
			Ok::<_, Error>(service_fn(move |req| {
				let db_pool = db_pool.clone();
				handle(req, db_pool)
			}))
		}
	});

	let server = Server::bind(&addr).serve(make_svc);
	
	println!("Builder Server listening on {}", addr.to_string().bright_blue());

	if let Err(e) = server.await {
		eprintln!("server error: {}", e);
	}
}