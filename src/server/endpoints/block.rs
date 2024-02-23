use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row as TRow;
use std::collections::HashMap;
use tracing::info;

use crate::{
    server::{
        blocks::{BlockInfoWithEpoch, HashID, TxShort},
        tx::TxDecoded,
        tx::TxInfo,
        ServerState,
    },
    BlockInfo, Error,
};
use namada_sdk::rpc::query_epoch_at_height;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum LatestBlock {
    LastBlock(Box<BlockInfoWithEpoch>),
    LatestBlocks(Vec<BlockInfoWithEpoch>),
}

async fn get_tx_hashes(
    state: &ServerState,
    block: &mut BlockInfo,
    hash: &[u8],
) -> Result<(), Error> {
    let rows = state.db.get_tx_hashes_block(hash).await?;

    let mut tx_hashes: Vec<TxShort> = vec![];
    for row in rows.iter() {
        println!("GET_TX_HASHES_ {:?}", row.columns());
        let hash_id = HashID(row.try_get("hash")?);
        let tx_type: String = row.try_get("tx_type")?;
        //
        let descriptive_type: String;
        if tx_type == "Decrypted" {
            let row = state.db.get_tx(&hash_id.0).await?;
            let Some(row) = row else {
                break;
            };
            let mut tx = TxInfo::try_from(row)?;

            // ignore the error for now
            _ = tx.decode_tx(&state.checksums_map);
            // println!("{:?}", tx.tx);
            descriptive_type = match tx.tx {
                Some(TxDecoded::Transfer(_)) => "Transfer".to_string(),
                Some(TxDecoded::Bond(_)) => "Bond".to_string(),
                Some(TxDecoded::RevealPK(_)) => "RevealPK".to_string(),
                Some(TxDecoded::VoteProposal(_)) => "VoteProposal".to_string(),
                Some(TxDecoded::BecomeValidator(_)) => "BecomeValidator".to_string(),
                Some(TxDecoded::InitValidator(_)) => "InitValidator".to_string(),
                Some(TxDecoded::Unbond(_)) => "Unbond".to_string(),
                Some(TxDecoded::Withdraw(_)) => "Withdraw".to_string(),
                Some(TxDecoded::InitAccount(_)) => "InitAccount".to_string(),
                Some(TxDecoded::UpdateAccount(_)) => "UpdateAccount".to_string(),
                Some(TxDecoded::ResignSteward(_)) => "ResignSteward".to_string(),
                Some(TxDecoded::UpdateStewardCommission(_)) => {
                    "UpdateStewardCommission".to_string()
                }
                Some(TxDecoded::EthPoolBridge(_)) => "EthPoolBridge".to_string(),
                Some(TxDecoded::Ibc(_)) => "Ibc".to_string(),
                Some(TxDecoded::ConsensusKeyChange(_)) => "ConsensusKeyChange".to_string(),
                Some(TxDecoded::CommissionChange(_)) => "CommissionChange".to_string(),
                Some(TxDecoded::MetaDataChange(_)) => "MetaDataChange".to_string(),
                Some(TxDecoded::ClaimRewards(_)) => "ClaimRewards".to_string(),
                Some(TxDecoded::DeactivateValidator(_)) => "DeactivateValidator".to_string(),
                Some(TxDecoded::ReactivateValidator(_)) => "ReactivateValidator".to_string(),
                Some(TxDecoded::UnjailValidator(_)) => "UnjailValidator".to_string(),
                Some(TxDecoded::InitProposal(_)) => "InitProposal".to_string(),
                _ => "Decrypted".to_string(),
            }
        } else {
            descriptive_type = "Wrapper".to_string();
        }

        tx_hashes.push(TxShort {
            tx_type: descriptive_type,
            hash_id,
        });
    }

    block.tx_hashes = tx_hashes;

    Ok(())
}

pub async fn get_block_by_hash(
    State(state): State<ServerState>,
    Path(hash): Path<String>,
) -> Result<Json<Option<BlockInfo>>, Error> {
    info!("calling /block/hash/:block_hash");

    let id = hex::decode(hash)?;

    let row = state.db.block_by_id(&id).await?;
    let Some(row) = row else {
        return Ok(Json(None));
    };
    let mut block = BlockInfo::try_from(&row)?;

    let block_id: Vec<u8> = row.try_get("block_id")?;
    get_tx_hashes(&state, &mut block, &block_id).await?;

    Ok(Json(Some(block)))
}

pub async fn get_block_by_height(
    State(state): State<ServerState>,
    Path(height): Path<u32>,
) -> Result<Json<Option<BlockInfo>>, Error> {
    info!("calling /block/height/:block_height");

    let row = state.db.block_by_height(height).await?;
    let Some(row) = row else {
        return Ok(Json(None));
    };

    let mut block = BlockInfo::try_from(&row)?;

    let block_id: Vec<u8> = row.try_get("block_id")?;
    get_tx_hashes(&state, &mut block, &block_id).await?;

    Ok(Json(Some(block)))
}

// TODO: indexing epoch for each block would be faster than querying node at request time
pub async fn get_last_block(
    State(state): State<ServerState>,
    Query(params): Query<HashMap<String, i32>>,
) -> Result<Json<LatestBlock>, Error> {
    info!("calling /block/last");

    let num = params.get("num");
    let offset = params.get("offset");

    if let Some(n) = num {
        let rows = state.db.get_lastest_blocks(n, offset).await?;
        let mut blocks: Vec<BlockInfoWithEpoch> = vec![];

        for row in rows {
            let mut block = BlockInfo::try_from(&row)?;

            let block_id: Vec<u8> = row.try_get("block_id")?;
            get_tx_hashes(&state, &mut block, &block_id).await?;

            let epoch = query_epoch_at_height(&state.http_client, block.header.height.into()).await?;

            let block_with_epoch = BlockInfoWithEpoch {
                block_id: block.block_id,
                header: block.header,
                last_commit: block.last_commit,
                tx_hashes: block.tx_hashes,
                epoch,
            };

            blocks.push(block_with_epoch);
        }

        Ok(Json(LatestBlock::LatestBlocks(blocks)))
    } else {
        let row = state.db.get_last_block().await?;

        let mut block = BlockInfo::try_from(&row)?;

        let block_id: Vec<u8> = row.try_get("block_id")?;
        get_tx_hashes(&state, &mut block, &block_id).await?;

        let epoch = query_epoch_at_height(&state.http_client, block.header.height.into()).await?;

        let block_with_epoch = Box::new(BlockInfoWithEpoch {
            block_id: block.block_id,
            header: block.header,
            last_commit: block.last_commit,
            tx_hashes: block.tx_hashes,
            epoch,
        });

        Ok(Json(LatestBlock::LastBlock(block_with_epoch)))
    }
}
