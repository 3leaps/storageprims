//! Existing JSON projections remain stable despite Rust enum extensibility.
use storageprims_core::*;

#[test]
fn existing_capabilities_and_inspection_dimensions_keep_their_json_names() {
    for (value, name) in [
        (Capability::DelimiterListing, "delimiter_listing"),
        (Capability::MultipartUpload, "multipart_upload"),
        (Capability::CredentialProbe, "credential_probe"),
        (Capability::ConditionalPut, "conditional_put"),
        (Capability::GuardedRead, "guarded_read"),
    ] {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json, name);
        assert_eq!(serde_json::from_value::<Capability>(json).unwrap(), value);
    }
    for (value, name) in [
        (InspectionLimitDimension::PayloadBytes, "payload_bytes"),
        (InspectionLimitDimension::Requests, "requests"),
        (InspectionLimitDimension::OutputBytes, "output_bytes"),
        (
            InspectionLimitDimension::PendingLineBytes,
            "pending_line_bytes",
        ),
        (InspectionLimitDimension::LineCount, "line_count"),
        (InspectionLimitDimension::ChunkSize, "chunk_size"),
        (InspectionLimitDimension::ProbeSize, "probe_size"),
        (InspectionLimitDimension::Deadline, "deadline"),
        (InspectionLimitDimension::Entries, "entries"),
    ] {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json, name);
        assert_eq!(
            serde_json::from_value::<InspectionLimitDimension>(json).unwrap(),
            value
        );
    }
}

#[test]
fn existing_source_receipt_and_default_put_options_keep_exact_shapes() {
    let receipt = SourceReceipt {
        path: "sample.bin".into(),
        native_version: None,
        validator: Some("synthetic-validator".into()),
        total_size: Some(4),
        requested_window: None,
        returned_window: None,
    };
    let expected = serde_json::json!({"path":"sample.bin", "native_version":null,
        "validator":"synthetic-validator", "total_size":4,
        "requested_window":null, "returned_window":null});
    assert_eq!(serde_json::to_value(&receipt).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<SourceReceipt>(expected).unwrap(),
        receipt
    );
    assert_eq!(
        serde_json::to_value(PutOptions::default()).unwrap(),
        serde_json::json!({"content_length":null,"content_type":null})
    );
}
