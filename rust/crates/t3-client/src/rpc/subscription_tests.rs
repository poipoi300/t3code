use super::*;
use futures::executor::block_on;
use serde_json::json;

#[test]
fn chunk_decode_defect_ends_the_request_before_later_values() {
    block_on(async {
        let mailbox = Rc::new(RefCell::new(Mailbox {
            chunks: VecDeque::from([vec![json!("not a number"), json!(7)]]),
            ..Default::default()
        }));
        let raw = RawCall {
            owner: Weak::new(),
            id: RequestId::String("decode".into()),
            mailbox,
            kind: CallKind::Stream,
            ended: false,
        };
        let mut stream = decoded_stream::<u64, _>(Ok(raw), |error| error);
        assert!(matches!(
            stream.next().await,
            Some(Err(RpcCallError::InvalidResponse))
        ));
        assert!(stream.next().await.is_none());
    });
}
