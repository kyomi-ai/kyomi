// Candle backend (pure Rust) — BGE-small-en-v1.5 via candle-transformers BERT.

use anyhow::Result;
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

pub struct CandleEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl CandleEmbedder {
    pub fn new() -> Result<Self> {
        let device = Device::Cpu;

        // Download model files from HuggingFace Hub (cached after first run)
        let api = Api::new()?;
        let repo = api.repo(Repo::new(
            "BAAI/bge-small-en-v1.5".to_string(),
            RepoType::Model,
        ));

        let config_path = repo.get("config.json")?;
        let tokenizer_path = repo.get("tokenizer.json")?;
        let weights_path = repo.get("model.safetensors")?;

        // Load config
        let config: Config = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;

        // Load weights via memory-mapped safetensors
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)?
        };

        let model = BertModel::load(vb, &config)?;

        // Set up tokenizer with padding (for batch processing)
        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("tokenizer load failed: {e}"))?;

        tokenizer.with_truncation(Some(TruncationParams {
            max_length: 512,
            ..Default::default()
        })).map_err(|e| anyhow::anyhow!("truncation config failed: {e}"))?;

        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize failed: {e}"))?;

        let token_ids = Tensor::new(encoding.get_ids(), &self.device)?.unsqueeze(0)?;
        let token_type_ids = token_ids.zeros_like()?;
        let attention_mask =
            Tensor::new(encoding.get_attention_mask(), &self.device)?.unsqueeze(0)?;

        let embeddings = self
            .model
            .forward(&token_ids, &token_type_ids, Some(&attention_mask))?;

        // CLS pooling (index 0) — matches BGE-small-en-v1.5 default
        let cls = embeddings.get(0)?.get(0)?;

        // L2 normalize
        let norm = cls.sqr()?.sum_all()?.sqrt()?;
        let normalized = cls.broadcast_div(&norm)?;

        Ok(normalized.to_vec1::<f32>()?)
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // Configure padding for this batch
        let mut tokenizer = self.tokenizer.clone();
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));

        let encodings = tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("batch tokenize failed: {e}"))?;

        let batch_size = encodings.len();

        // Build padded tensors
        let token_ids: Vec<&[u32]> = encodings.iter().map(|e| e.get_ids()).collect();
        let token_type_ids_data: Vec<Vec<u32>> =
            encodings.iter().map(|e| vec![0u32; e.get_ids().len()]).collect();
        let attention_masks: Vec<&[u32]> =
            encodings.iter().map(|e| e.get_attention_mask()).collect();

        let token_ids = Tensor::new(token_ids, &self.device)?;
        let token_type_ids_refs: Vec<&[u32]> =
            token_type_ids_data.iter().map(|v| v.as_slice()).collect();
        let token_type_ids = Tensor::new(token_type_ids_refs, &self.device)?;
        let attention_mask = Tensor::new(attention_masks, &self.device)?;

        let embeddings =
            self.model
                .forward(&token_ids, &token_type_ids, Some(&attention_mask))?;

        // CLS pooling + L2 normalize for each item
        let mut results = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let cls = embeddings.get(i)?.get(0)?;
            let norm = cls.sqr()?.sum_all()?.sqrt()?;
            let normalized = cls.broadcast_div(&norm)?;
            results.push(normalized.to_vec1::<f32>()?);
        }

        Ok(results)
    }
}
