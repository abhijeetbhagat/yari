use crate::raft::{ElectionThread, Raft, ServerMessageOrStateMachineMessage, StateMachine};

use crate::rpc::{AppendRequest, ClientRequest, VoteRequest};
use async_std::prelude::*;
use async_std::sync::{Arc, RwLock};
use serde_json::json;
use tide::{Body, Request, StatusCode};
use url::Url;

async fn append<SM: StateMachine>(mut request: Request<WebState<SM>>) -> tide::Result {
    let request_body: AppendRequest<ServerMessageOrStateMachineMessage<SM::MessageType>> =
        request.body_json().await?;
    let state = request.state().clone();
    let mut state = state.write().await;
    Ok(Body::from_json(&state.append(request_body).await)?.into())
}

async fn vote<SM: StateMachine>(mut request: Request<WebState<SM>>) -> tide::Result {
    let vote_request: VoteRequest = request.body_json().await?;
    let state = request.state().clone();
    let mut state = state.write().await;
    Ok(Body::from_json(&state.vote(vote_request).await)?.into())
}

async fn client<SM: StateMachine>(mut request: Request<WebState<SM>>) -> tide::Result {
    let client_request: ClientRequest<SM::MessageType> = request.body_json().await?;
    let state = request.state().clone();
    let mut state = state.write().await;
    let result = state.client(client_request).await;

    match result {
        Err(Some(leader)) => Ok(tide::Redirect::new(Url::parse(&leader)?.join("/client")?).into()),
        Err(None) => Ok(tide::StatusCode::ServiceUnavailable.into()),
        Ok(le) => {
            let result = le.recv().await?;
            Ok(json!({ "result": result }).into())
        }
    }
}

fn leader_redirect<SM: StateMachine>(id: &str, raft: &Raft<SM>) -> tide::Result {
    match raft.leader_id_for_client_redirection() {
        Some(redirect) => Ok(tide::Redirect::new(
            Url::parse(&id)?
                .join("/servers/")?
                .join(&urlencoding::encode(redirect))?,
        )
        .into()),

        None => Ok(StatusCode::InternalServerError.into()),
    }
}

async fn add_server<SM: StateMachine>(request: Request<WebState<SM>>) -> tide::Result {
    let raft = request.state().clone();
    let mut raft = raft.write().await;
    let id: String = request.param("id")?;

    if raft.is_leader() {
        raft.member_add(&urlencoding::decode(&id)?).await;
        Ok(tide::StatusCode::Ok.into())
    } else {
        leader_redirect(&id, &*raft)
    }
}

async fn remove_server<SM: StateMachine>(request: Request<WebState<SM>>) -> tide::Result {
    let raft = request.state().clone();
    let mut raft = raft.write().await;
    let id: String = request.param("id")?;

    if raft.is_leader() {
        raft.member_remove(&urlencoding::decode(&id)?).await;
        Ok(tide::StatusCode::Ok.into())
    } else {
        leader_redirect(&id, &*raft)
    }
}

// async fn index<SM: StateMachine>(_r: Request<WebState<SM>>) -> tide::Result {
//     Ok(tide::Body::from_file("./web/build/index.html")
//         .await?
//         .into())
// }

// async fn sse<SM: StateMachine>(r: Request<WebState<SM>>) -> tide::Result {
//     eprintln!("sse connected");
//     let state = r.state().clone();
//     let channel = state.read().await.channel.clone();
//     let mut response = channel.into_response();
//     response.insert_header(ACCESS_CONTROL_ALLOW_ORIGIN, "*");
//     Ok(response)
// }

type WebState<SM> = Arc<RwLock<Raft<SM>>>;

pub async fn start<SM, L>(state: Arc<RwLock<Raft<SM>>>, address: L) -> Result<(), std::io::Error>
where
    SM: StateMachine,
    L: tide::listener::ToListener<WebState<SM>>,
{
    log::info!("start");
    let et_state = state.clone();
    let et = async_std::task::spawn(async move {
        log::info!("spawning election thread");
        ElectionThread::spawn(et_state).await;
        Ok(())
    });

    let mut server = tide::with_state(state);
    server.at("/append").post(append::<SM>);
    server
        .at("/vote")
        .with(driftwood::DevLogger)
        .post(vote::<SM>);
    server
        .at("/client")
        .with(driftwood::DevLogger)
        .post(client::<SM>);
    // server.at("/sse").get(sse::<SM>);
    server
        .at("/servers/:id")
        .with(driftwood::DevLogger)
        .put(add_server::<SM>)
        .delete(remove_server::<SM>);
    //    server.at("/").get(index::<SM>).serve_dir("./web/build")?;
    log::info!("about to listen");
    et.race(server.listen(address)).await?;
    log::info!("done listening?");

    Ok(())
}
