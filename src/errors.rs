use thiserror::Error;

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