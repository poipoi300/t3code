use serde_json::{Value, json};
use t3_orchestration::ports::transport::tool_output_image_bytes::*;
fn strip(mut value: Value) -> Value {
    strip_unserved_output_image_bytes(&mut value);
    value
}
#[test]
fn strips_structured_read_base64_and_measures_it() {
    let data = "B".repeat(4000);
    for size in [None, Some(0)] {
        let mut input = json!({"type":"image","file":{"base64":data,"type":"image/png","originalSize":3000,"dimensions":{"width":180}}});
        if let Some(size) = size {
            input["file"]["sizeBytes"] = size.into();
        }
        assert_eq!(
            strip(input),
            json!({"type":"image","file":{"type":"image/png","originalSize":3000,"dimensions":{"width":180},"sizeBytes":3000}})
        );
    }
}
#[test]
fn strips_acp_images_and_unserved_results_behind_errors() {
    assert_eq!(
        strip(
            json!({"rawOutput":{"type":"ReadFile","ImageContent":{"data":"B".repeat(4000),"mime_type":"image/png"}},"failed":{"error":"partial","result":[{"type":"image","data":format!("{}==","C".repeat(4002)),"mimeType":"image/png"}]}})
        ),
        json!({"rawOutput":{"type":"ReadFile","ImageContent":{"mime_type":"image/png","sizeBytes":3000}},"failed":{"error":"partial","result":[{"type":"image","mimeType":"image/png","sizeBytes":3001}]}})
    );
}
#[test]
fn strips_cursor_image_parts_without_mime() {
    assert_eq!(
        strip(
            json!({"content":[{"text":{"text":"captured"}},{"image":{"data":"A".repeat(4000)}}]})
        ),
        json!({"content":[{"text":{"text":"captured"}},{"image":{"sizeBytes":3000}}]})
    );
}
#[test]
fn keeps_only_the_served_image_positions_within_the_size_limit() {
    let claude = json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"A".repeat(4000)}});
    let mcp = json!({"type":"image","mimeType":"image/jpeg","data":"B".repeat(4000)});
    let value = json!({"content":[{"type":"text","text":"captured"},claude,mcp]});
    assert_eq!(strip(value.clone()), value);
    let images = vec![json!({"type":"image","data":"A".repeat(4000),"mimeType":"image/png"}); 10];
    let result = strip(json!(images));
    assert_eq!(&result.as_array().unwrap()[..8], &images[..8]);
    assert_eq!(
        result[8],
        json!({"type":"image","mimeType":"image/png","sizeBytes":3000})
    );
    assert_eq!(result[9], result[8]);
}
#[test]
fn strips_an_oversized_served_position_but_keeps_the_following_image() {
    let image = json!({"type":"image","mimeType":"image/png","data":"A".repeat(4000)});
    let result = strip(
        json!({"content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"A".repeat(MAX_TOOL_OUTPUT_IMAGE_BASE64_LENGTH+4)}},image]}),
    );
    assert_eq!(
        result["content"][0]["source"],
        json!({"type":"base64","media_type":"image/png","sizeBytes":(MAX_TOOL_OUTPUT_IMAGE_BASE64_LENGTH+4)/4*3})
    );
    assert_eq!(result["content"][1], image);
}
#[test]
fn keeps_non_base64_and_non_image_data_and_preserves_untouched_siblings() {
    let data = json!({"nested":{"byId":{"type":"image","data":"asset-id-42"},"svg":{"mimeType":"image/svg+xml","data":format!("<svg>{}</svg>","<circle/>".repeat(30))},"dataUrl":{"type":"image","data":format!("data:image/png;base64,{}","A".repeat(4000))},"remote":{"type":"image","source":{"type":"url","url":"https://example.test/image"}},"tiny":{"type":"image","data":"AA==","mimeType":"image/png"},"notImage":{"mimeType":"application/pdf","data":"A".repeat(4000)}}});
    assert_eq!(strip(data.clone()), data);
    let original = json!({"error":"partial","result":[{"type":"text","text":"captured"},{"type":"image","data":"A".repeat(4000),"mimeType":"image/png"}]});
    let stripped = strip(original.clone());
    assert_eq!(stripped["result"][0], original["result"][0]);
    assert!(original["result"][1].get("data").is_some());
}
