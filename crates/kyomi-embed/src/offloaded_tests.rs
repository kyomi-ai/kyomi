//! Preserve asymmetric encoding through the async single-text API.
use super::EmbeddingService;

#[tokio::test]
async fn offloaded_single_text_preserves_passage_and_query_encoding() {
    let svc = EmbeddingService::new().expect("load real embedding model");
    let passage = svc
        .embed_passage("revenue by region")
        .expect("sync passage");
    let query = svc.embed_query("revenue by region").expect("sync query");
    assert_ne!(passage, query, "BGE query prefix must affect the embedding");
    assert_eq!(
        svc.embed_passage_offloaded("revenue by region")
            .await
            .expect("offloaded passage"),
        passage
    );
    assert_eq!(
        svc.embed_query_offloaded("revenue by region")
            .await
            .expect("offloaded query"),
        query
    );
}
