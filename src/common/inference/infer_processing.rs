use std::collections::{HashMap, HashSet};

use api::rest::models::InferenceUsage;
use collection::operations::point_ops::VectorPersisted;
use storage::content_manager::errors::StorageError;

use super::batch_processing::BatchAccum;
use super::service::{
    InferenceData, InferenceInput, InferenceResponse, InferenceService, InferenceType,
};
use crate::common::inference::InferenceToken;

pub struct BatchAccumInferred {
    pub(crate) objects: HashMap<InferenceData, VectorPersisted>,
}

impl BatchAccumInferred {
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
        }
    }

    pub async fn from_objects(
        objects: HashSet<InferenceData>,
        inference_type: InferenceType,
        inference_token: InferenceToken,
    ) -> Result<(Self, Option<InferenceUsage>), StorageError> {
        if objects.is_empty() {
            return Ok((Self::new(), None));
        }

        let Some(service) = InferenceService::get_global() else {
            return Err(StorageError::service_error(
                "InferenceService is not initialized. Please check if it was properly configured and initialized during startup.",
            ));
        };

        service.validate()?;

        let objects_serialized: Vec<_> = objects.into_iter().collect();
        let inference_inputs: Vec<_> = objects_serialized
            .iter()
            .cloned()
            .map(InferenceInput::from)
            .collect();

        let InferenceResponse { embeddings, usage } = service
            .infer(inference_inputs, inference_type, inference_token)
            .await?;

        if embeddings.is_empty() {
            return Err(StorageError::service_error(
                "Inference service returned no vectors. Check if models are properly loaded.",
            ));
        }

        let objects = objects_serialized.into_iter().zip(embeddings).collect();

        Ok((Self { objects }, usage))
    }

    pub async fn from_batch_accum(
        batch: BatchAccum,
        inference_type: InferenceType,
        inference_token: &InferenceToken,
    ) -> Result<(Self, Option<InferenceUsage>), StorageError> {
        let BatchAccum { objects } = batch;
        Self::from_objects(objects, inference_type, inference_token.clone()).await
    }

    pub fn get_vector(&self, data: &InferenceData) -> Option<&VectorPersisted> {
        self.objects.get(data)
    }
}
