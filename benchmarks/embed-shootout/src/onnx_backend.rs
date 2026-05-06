// ONNX Runtime backend via fastembed — mirrors production kyomi-embed setup.

use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

pub struct OnnxEmbedder {
    model: TextEmbedding,
}

impl OnnxEmbedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGESmallENV15))?;
        Ok(Self { model })
    }

    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let mut results = self.model.embed(vec![text], None)?;
        Ok(results.pop().unwrap())
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        Ok(self.model.embed(texts, None)?)
    }
}
