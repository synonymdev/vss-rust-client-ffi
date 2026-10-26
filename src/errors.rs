use thiserror::Error;

/// Conditional writes never replay a mutation automatically. An unknown outcome
/// requires reading the candidate keys before deciding whether another write is safe.
#[derive(Error, Debug, uniffi::Error)]
pub enum ConditionalWriteError {
    #[error("VSS client is not initialized")]
    ClientNotInitialized,
    #[error("Invalid conditional write: {reason}")]
    InvalidInput { reason: String },
    #[error("A key version changed; the transaction was not applied")]
    Conflict,
    #[error("The server rejected the conditional write")]
    Rejected,
    #[error("The write outcome is unknown; reconcile the saved candidate before retrying")]
    OutcomeUnknown,
}

#[derive(Error, Debug, uniffi::Error)]
pub enum VssError {
    #[error("Connection error: {error_details}")]
    ConnectionError { error_details: String },

    #[error("Authentication error: {error_details}")]
    AuthError { error_details: String },

    #[error("Store error: {error_details}")]
    StoreError { error_details: String },

    #[error("Get error: {error_details}")]
    GetError { error_details: String },

    #[error("List error: {error_details}")]
    ListError { error_details: String },

    #[error("Put error: {error_details}")]
    PutError { error_details: String },

    #[error("Delete error: {error_details}")]
    DeleteError { error_details: String },

    #[error("Invalid data: {error_details}")]
    InvalidData { error_details: String },

    #[error("Network error: {error_details}")]
    NetworkError { error_details: String },

    #[error("Unknown error: {error_details}")]
    UnknownError { error_details: String },
}
