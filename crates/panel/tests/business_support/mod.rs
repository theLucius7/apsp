#![allow(dead_code)]

use crate::release_support;

use anyhow::{Context, Result, bail};
use apsp_panel::{AppState, config::Config, router};
use apsp_protocol::{
    AuthChallenge, AuthResponse, EnrollRequest, Envelope, Hello, HelloAck, PROTOCOL_VERSION,
    StaticInfo,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method, Response, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use uuid::Uuid;

pub mod deployment;
#[path = "../release_fixture/mod.rs"]
pub mod release_fixture;

pub type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Marks the admin session as recently re-authenticated, which applying an operation or
/// restarting a runtime requires.
pub async fn prove(pool: &PgPool, cookie: &str) -> Result<()> {
    let token = cookie
        .strip_prefix("apsp_session=")
        .context("session cookie")?;
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=$2,expires_at=$3")
        .bind(apsp_panel::auth::hash_token(token))
        .bind(now)
        .bind(now + 300)
        .execute(pool)
        .await?;
    Ok(())
}
const PASSWORD: &str = "business-test-password";

pub struct TestPanel {
    pub state: AppState,
    pub client: Client,
    pub base: String,
    directory: PathBuf,
    task: JoinHandle<()>,
}

impl TestPanel {
    pub async fn start(pool: PgPool) -> Result<Self> {
        Self::start_with_public_url(pool, None).await
    }

    pub async fn start_with_public_url(pool: PgPool, public_url: Option<&str>) -> Result<Self> {
        Self::start_configured(pool, public_url, false).await
    }

    pub async fn start_with_localhost(pool: PgPool) -> Result<Self> {
        Self::start_configured(pool, None, true).await
    }

    async fn start_configured(
        pool: PgPool,
        public_url: Option<&str>,
        localhost: bool,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen = listener.local_addr()?;
        let base = format!("http://{listen}");
        let directory = std::env::temp_dir().join(format!("apsp-business-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let mut state = AppState::new(
            pool,
            Config {
                database_url: String::new(),
                listen,
                public_url: public_url.map(str::to_owned).unwrap_or_else(|| {
                    if localhost {
                        format!("http://localhost:{}", listen.port())
                    } else {
                        base.clone()
                    }
                }),
                data_dir: directory.clone(),
                admin_password: Some(PASSWORD.into()),
            },
        )
        .await?;
        state.release_keys = Some(Arc::new(release_support::trusted_keys()));
        state.domain_states = state
            .domain_states
            .with(apsp_panel::ip_quality::ProviderRegistry::default());
        let app = router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("test HTTP server")
        });
        Ok(Self {
            state,
            base,
            directory,
            task,
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()?,
        })
    }

    pub async fn admin_cookie(&self) -> Result<String> {
        let response = self
            .client
            .post(format!("{}/api/login", self.base))
            .json(&json!({"password":PASSWORD}))
            .send()
            .await?
            .error_for_status()?;
        Ok(response
            .headers()
            .get(header::SET_COOKIE)
            .context("session header")?
            .to_str()?
            .split(';')
            .next()
            .context("session cookie")?
            .into())
    }

    pub async fn admin(
        &self,
        method: Method,
        path: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> Result<Response> {
        let request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header(header::COOKIE, cookie);
        Ok(match body {
            Some(body) => request.json(&body),
            None => request,
        }
        .send()
        .await?)
    }

    pub async fn create_server(&self, cookie: &str, name: &str) -> Result<i64> {
        let response = self
            .admin(
                Method::POST,
                "/api/servers",
                cookie,
                Some(json!({"name":name})),
            )
            .await?;
        anyhow::ensure!(
            response.status() == StatusCode::CREATED,
            "server creation failed: {}",
            response.text().await?
        );
        id(&response.json::<Value>().await?)
    }

    pub async fn create_node(&self, cookie: &str, server_id: i64, name: &str) -> Result<Value> {
        self.enable_plugin(cookie, server_id).await?;
        let response = self.admin(Method::POST, "/api/plugins/sing-box/nodes", cookie, Some(json!({"name":name,"server_id":server_id,"public_host":"proxy.example.com","sni":"www.example.com"}))).await?;
        anyhow::ensure!(
            response.status() == StatusCode::CREATED,
            "node creation failed: {}",
            response.text().await?
        );
        Ok(response.json().await?)
    }

    /// Explicit TEST_ONLY import preserving both numeric and rich legacy projections.
    pub async fn import_legacy_chain(
        &self,
        cookie: &str,
        name: &str,
        entry: i64,
        exit: i64,
    ) -> Result<Value> {
        let mut tx = self.state.pool.begin().await?;
        let relay = Uuid::new_v4();
        let chain:i64=sqlx::query_scalar("INSERT INTO singbox_chains(name,entry_node_id,exit_node_id,relay_uuid,applied_generation) VALUES($1,$2,$3,$4,1) RETURNING id")
            .bind(name).bind(entry).bind(exit).bind(relay).fetch_one(&mut *tx).await?;
        let mut endpoints = Vec::new();
        for node in [entry, exit] {
            let (server,snapshot):(i64,Value)=sqlx::query_as("SELECT server_id,jsonb_build_object('id',id,'name',name,'port',port,'public_host',public_host,'sni',sni,'private_key',private_key,'public_key',public_key,'short_id',short_id,'users','[]'::jsonb,'enabled',enabled,'settings',settings,'protocol_config',protocol_config) FROM nodes WHERE id=$1")
                .bind(node).fetch_one(&mut *tx).await?;
            let proposed = Uuid::new_v4();
            sqlx::query("INSERT INTO singbox_managed_endpoint_versions(id,node_id,server_id,snapshot,semantic_sha256,created_at) VALUES($1,$2,$3,$4,encode(sha256(convert_to($4::jsonb::text,'UTF8')),'hex'),$5) ON CONFLICT(node_id,semantic_sha256) DO NOTHING")
                .bind(proposed).bind(node).bind(server).bind(&snapshot).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
            let version:Uuid=sqlx::query_scalar("SELECT id FROM singbox_managed_endpoint_versions WHERE node_id=$1 AND semantic_sha256=encode(sha256(convert_to($2::jsonb::text,'UTF8')),'hex')")
                .bind(node).bind(&snapshot).fetch_one(&mut *tx).await?;
            endpoints.push(json!({"version_id":version,"server_id":server,"node":snapshot}));
        }
        let frozen = json!({"entry":endpoints[0],"hops":[{"kind":"managed","endpoint":endpoints[1],"relay_uuid":relay}],"legacy_relay_uuid":relay});
        let entry_version: Uuid = serde_json::from_value(endpoints[0]["version_id"].clone())?;
        let exit_version: Uuid = serde_json::from_value(endpoints[1]["version_id"].clone())?;
        let exit_server = endpoints[1]["server_id"]
            .as_i64()
            .context("legacy fixture exit server")?;
        sqlx::query("INSERT INTO singbox_ordered_chain_versions(chain_id,generation,legacy,entry_endpoint_version,semantic_sha256,capabilities,snapshot,created_at) VALUES($1,1,TRUE,$2,encode(sha256(convert_to($3::jsonb::text,'UTF8')),'hex'),'{\"tcp\":true,\"udp\":true}'::jsonb,$3,$4)")
            .bind(chain).bind(entry_version).bind(frozen).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO singbox_ordered_chain_hops(chain_id,generation,position,kind,endpoint_version_id,managed_node_id,managed_server_id,relay_uuid) VALUES($1,1,1,'managed',$2,$3,$4,$5)")
            .bind(chain).bind(exit_version).bind(exit).bind(exit_server).bind(relay).execute(&mut *tx).await?;
        let main_path = json!({"chain_id":chain,"generation":1,"entry_server_id":endpoints[0]["server_id"],"entry_node_id":entry,"active":true,"hops":[{"kind":"managed","server_id":exit_server,"identity":relay,"endpoint":endpoints[1]["node"]}]});
        sqlx::query("INSERT INTO singbox_chain_versions(chain_id,generation,legacy,path_json,semantic_hash,networks,stage,created_at,updated_at) VALUES($1,1,TRUE,$2,'legacy-preserved','{\"tcp\":true,\"udp\":true}','active',0,0)")
            .bind(chain).bind(&main_path).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,managed_node_id,managed_server_id,endpoint_json,relay_uuid) VALUES($1,1,0,'managed',$2,$3,$4,$5)")
            .bind(chain).bind(exit).bind(exit_server).bind(&endpoints[1]["node"]).bind(relay).execute(&mut *tx).await?;
        sqlx::query("UPDATE singbox_chains SET active_generation=1 WHERE id=$1")
            .bind(chain)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE servers SET dirty_at=$2 WHERE id IN (SELECT server_id FROM nodes WHERE id=ANY($1))")
            .bind(vec![entry,exit]).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
        tx.commit().await?;
        let list: Value = self
            .admin(Method::GET, "/api/plugins/sing-box/chains", cookie, None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        list.as_array()
            .context("legacy fixture chain list")?
            .iter()
            .find(|value| value["id"] == chain)
            .cloned()
            .context("legacy fixture chain presentation")
    }

    pub async fn enable_plugin(&self, cookie: &str, server_id: i64) -> Result<()> {
        self.admin(
            Method::POST,
            &format!("/api/plugins/sing-box/servers/{server_id}/enable"),
            cookie,
            Some(json!({})),
        )
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn create_user(&self, cookie: &str, name: &str) -> Result<Value> {
        let response = self
            .admin(
                Method::POST,
                "/api/plugins/sing-box/users",
                cookie,
                Some(json!({"name":name})),
            )
            .await?;
        anyhow::ensure!(
            response.status() == StatusCode::CREATED,
            "user creation failed: {}",
            response.text().await?
        );
        Ok(response.json().await?)
    }

    /// Gives a user one node the only way the panel allows: through a policy group
    /// that holds just that node. Returns the user's access row for it.
    pub async fn grant(&self, cookie: &str, user_id: i64, node_id: i64) -> Result<Value> {
        let group = self.node_group(cookie, node_id).await?;
        let mut groups = self.user_groups(cookie, user_id).await?;
        if !groups.contains(&group) {
            groups.push(group);
        }
        self.set_user_groups(cookie, user_id, &groups).await?;
        let accesses: Vec<Value> = self
            .admin(
                Method::GET,
                &format!("/api/plugins/sing-box/users/{user_id}/accesses"),
                cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        accesses
            .into_iter()
            .find(|access| access["node_id"] == node_id)
            .ok_or_else(|| anyhow::anyhow!("node {node_id} was not granted to user {user_id}"))
    }

    /// Takes the single-node group of [`grant`](Self::grant) away from the user again.
    pub async fn revoke(&self, cookie: &str, user_id: i64, node_id: i64) -> Result<()> {
        let group = self.node_group(cookie, node_id).await?;
        let groups: Vec<i64> = self
            .user_groups(cookie, user_id)
            .await?
            .into_iter()
            .filter(|id| *id != group)
            .collect();
        self.set_user_groups(cookie, user_id, &groups).await?;
        // An unused single-node group would still reserve the node, as any group does.
        let unused = self
            .admin(
                Method::GET,
                "/api/plugins/sing-box/policy-groups",
                cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json::<Vec<Value>>()
            .await?
            .iter()
            .any(|value| value["id"] == group && value["member_count"] == 0);
        if unused {
            self.admin(
                Method::DELETE,
                &format!("/api/plugins/sing-box/policy-groups/{group}"),
                cookie,
                None,
            )
            .await?
            .error_for_status()?;
        }
        Ok(())
    }

    async fn node_group(&self, cookie: &str, node_id: i64) -> Result<i64> {
        let name = format!("node-{node_id}");
        let groups: Vec<Value> = self
            .admin(
                Method::GET,
                "/api/plugins/sing-box/policy-groups",
                cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(id) = groups
            .iter()
            .find(|group| group["name"] == name.as_str())
            .and_then(|group| group["id"].as_i64())
        {
            return Ok(id);
        }
        let response = self
            .admin(
                Method::POST,
                "/api/plugins/sing-box/policy-groups",
                cookie,
                Some(json!({"name":name,"node_ids":[node_id],"chain_ids":[]})),
            )
            .await?;
        anyhow::ensure!(
            response.status() == StatusCode::CREATED,
            "policy group creation failed: {}",
            response.text().await?
        );
        let group: Value = response.json().await?;
        group["id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("policy group without id"))
    }

    async fn user_groups(&self, cookie: &str, user_id: i64) -> Result<Vec<i64>> {
        let value: Value = self
            .admin(
                Method::GET,
                &format!("/api/plugins/sing-box/users/{user_id}/policy-groups"),
                cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(value["group_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect())
    }

    async fn set_user_groups(&self, cookie: &str, user_id: i64, groups: &[i64]) -> Result<()> {
        self.admin(
            Method::PUT,
            &format!("/api/plugins/sing-box/users/{user_id}/policy-groups"),
            cookie,
            Some(json!({"group_ids":groups})),
        )
        .await?
        .error_for_status()?;
        Ok(())
    }

    pub async fn publish_now(&self) -> Result<()> {
        self.prepare_deployment_preflights().await?;
        sqlx::query("UPDATE servers SET dirty_at=0 WHERE dirty_at IS NOT NULL")
            .execute(&self.state.pool)
            .await?;
        apsp_panel::publisher::publish_due(&self.state).await
    }

    /// Complete the normal deployment gate with controlled TEST_ONLY observations.
    pub async fn prepare_deployment_preflights(&self) -> Result<()> {
        deployment::prepare(self).await
    }

    pub async fn authenticated_device(
        &self,
        cookie: &str,
        name: &str,
    ) -> Result<(i64, Socket, HelloAck)> {
        let (server, socket, ack, _) = self.authenticated_device_with_key(cookie, name).await?;
        Ok((server, socket, ack))
    }

    pub async fn authenticated_device_with_key(
        &self,
        cookie: &str,
        name: &str,
    ) -> Result<(i64, Socket, HelloAck, SigningKey)> {
        let server_id = self.create_server(cookie, name).await?;
        let value: Value = self
            .admin(
                Method::POST,
                &format!("/api/servers/{server_id}/enrollment"),
                cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        self.client
            .post(format!("{}/api/agent/v1/enroll", self.base))
            .json(&EnrollRequest {
                token: value["token"].as_str().context("enrollment token")?.into(),
                device_public_key: URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
                static_info: StaticInfo {
                    arch: Some("amd64".into()),
                    hostname: Some("business-test".into()),
                    ..StaticInfo::default()
                },
            })
            .send()
            .await?
            .error_for_status()?;
        let (mut socket, _) = connect_async(format!(
            "{}/api/agent/v1/ws",
            self.base.replacen("http", "ws", 1)
        ))
        .await?;
        let challenge = receive_envelope(&mut socket).await?;
        anyhow::ensure!(
            challenge.message_type == "auth.challenge",
            "missing device challenge"
        );
        let challenge: AuthChallenge = challenge.to_payload()?;
        send_envelope(
            &mut socket,
            Envelope::new(
                "auth.response",
                AuthResponse {
                    server_id,
                    signature: URL_SAFE_NO_PAD
                        .encode(key.sign(challenge.nonce.as_bytes()).to_bytes()),
                },
            )?,
        )
        .await?;
        let ack = receive_envelope(&mut socket).await?;
        anyhow::ensure!(ack.message_type == "hello.ack", "missing device session");
        send_envelope(
            &mut socket,
            Envelope::new(
                "hello",
                Hello {
                    agent_version: "business-test".into(),
                    protocol_version: PROTOCOL_VERSION,
                    capabilities: vec![
                        apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY.into(),
                    ],
                    applied: BTreeMap::new(),
                },
            )?,
        )
        .await?;
        socket
            .send(Message::Ping(b"introduced".to_vec().into()))
            .await?;
        timeout(Duration::from_secs(5), async {
            loop {
                match socket
                    .next()
                    .await
                    .context("WebSocket ended before hello barrier")??
                {
                    Message::Pong(bytes) if bytes.as_ref() == b"introduced" => {
                        return Ok::<_, anyhow::Error>(());
                    }
                    Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
                    Message::Pong(_) => {}
                    other => bail!("unexpected frame before hello barrier: {other:?}"),
                }
            }
        })
        .await??;
        Ok((server_id, socket, ack.to_payload()?, key))
    }
}

impl Drop for TestPanel {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub fn id(value: &Value) -> Result<i64> {
    value["id"].as_i64().context("object id")
}

pub async fn send_envelope(socket: &mut Socket, envelope: Envelope) -> Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(&envelope)?.into()))
        .await?;
    Ok(())
}

pub async fn receive_envelope(socket: &mut Socket) -> Result<Envelope> {
    timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.context("WebSocket ended")?? {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
                Message::Pong(_) => {}
                other => bail!("expected envelope, got {other:?}"),
            }
        }
    })
    .await?
}
