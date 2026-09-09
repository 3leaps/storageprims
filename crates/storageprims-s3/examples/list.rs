use storageprims_core::{
    CredentialSource, ListOptions, ProviderConfig, ProviderKind, StorageProvider, StorageUri,
    TargetConfig,
};
use storageprims_s3::S3Provider;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let uri = StorageUri::parse("s3://my-bucket/data/")?;
    let config = ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig::default(),
        credentials: CredentialSource::DefaultChain,
    };
    let provider = S3Provider::from_uri(&uri, config).await?;

    let result = provider.list(ListOptions::default()).await?;
    println!("{} objects", result.objects.len());

    Ok(())
}
