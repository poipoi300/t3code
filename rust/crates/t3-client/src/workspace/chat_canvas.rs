//! Pure placement from chat/chatCanvasLayout.ts and chat/threadDetailsCardLayout.ts.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailsCardPlacement {
    pub x: f32,
    pub right: f32,
    pub width: f32,
    pub y: f32,
    pub height: f32,
}

pub const DETAILS_CARD_CLEARANCE: f32 = 32.0;

pub fn resolve_thread_details_card_layout(
    container: (f32, f32),
    lane: (f32, f32),
    frame: Option<Frame>,
    overlaps_details_card: bool,
    top_inset: f32,
) -> Option<DetailsCardPlacement> {
    let gap = 12.0;
    let width = 280.0;
    let x = container.0 - width - gap;
    if x - DETAILS_CARD_CLEARANCE - lane.0 < lane.1 {
        return None;
    }
    let y = gap + top_inset;
    let height = match frame {
        Some(frame)
            if overlaps_details_card
                && frame.x + frame.width > x - gap
                && frame.x < x + width + gap =>
        {
            (container.1 - y - gap).min(frame.y - y - gap)
        }
        _ => container.1 - y - gap,
    };
    (height >= 160.0).then_some(DetailsCardPlacement {
        x,
        right: gap,
        width,
        y,
        height,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatLane {
    pub left: f32,
    pub width: f32,
    pub inset_end: f32,
}

/// `resolveChatCanvasLayout`'s branch before the separately owned preview mini-player is mounted.
pub fn resolve_chat_canvas_lane(
    container_width: f32,
    padding: f32,
    max_chat_width: f32,
    details_card_left: Option<f32>,
) -> ChatLane {
    let centered_width = max_chat_width.min(container_width - padding * 2.0).max(0.0);
    let lane_right = details_card_left
        .map(|left| left - DETAILS_CARD_CLEARANCE)
        .unwrap_or(container_width - padding);
    let width = centered_width.min(lane_right - padding).max(0.0);
    let canvas_center_left = (container_width - width) / 2.0;
    let left = padding.max(canvas_center_left.min(lane_right - width));
    ChatLane {
        left,
        width,
        inset_end: (container_width - left * 2.0 - width).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_card_keeps_the_source_width_clearance_and_minimum_height_boundaries() {
        for (width, height, expected) in [
            (1000.0, 800.0, false),
            (1011.0, 800.0, false),
            (1012.0, 800.0, true),
            (1280.0, 183.0, false),
            (1280.0, 184.0, true),
        ] {
            assert_eq!(
                resolve_thread_details_card_layout(
                    (width, height),
                    (48.0, 640.0),
                    None,
                    false,
                    0.0
                )
                .is_some(),
                expected
            );
        }
        let frame = Frame {
            x: 980.0,
            y: 260.0,
            width: 288.0,
            height: 300.0,
        };
        let placement = resolve_thread_details_card_layout(
            (1280.0, 800.0),
            (48.0, 640.0),
            Some(frame),
            true,
            0.0,
        );
        assert_eq!(
            placement,
            Some(DetailsCardPlacement {
                x: 988.0,
                right: 12.0,
                width: 280.0,
                y: 12.0,
                height: 236.0
            })
        );
    }

    #[test]
    fn the_chat_lane_reserves_only_the_open_measured_card_and_stays_centered_otherwise() {
        // chatCanvasLayout.test.ts: unobstructed center, left shift, then minimum readable lane.
        for (container, card_left, left, width) in [
            (1344.0, 1052.0, 284.0, 736.0),
            (2200.0, 1876.0, 732.0, 736.0),
            (1500.0, 1208.0, 382.0, 736.0),
            (1108.0, 816.0, 48.0, 736.0),
            (1028.0, 736.0, 48.0, 656.0),
            (1012.0, 720.0, 48.0, 640.0),
        ] {
            let lane = resolve_chat_canvas_lane(container, 48.0, 736.0, Some(card_left));
            assert_eq!((lane.left, lane.width), (left, width));
        }
        for width in 1012..=1440 {
            let width = width as f32;
            let card =
                resolve_thread_details_card_layout((width, 900.0), (48.0, 640.0), None, false, 0.0)
                    .expect("source inline range");
            let lane = resolve_chat_canvas_lane(width, 48.0, 736.0, Some(card.x));
            assert!(lane.left >= 48.0 && lane.width >= 640.0);
            assert!(card.x - lane.left - lane.width >= 32.0);
        }
        for width in 1300..1800 {
            let left_at =
                |width: f32| resolve_chat_canvas_lane(width, 48.0, 736.0, Some(width - 292.0)).left;
            assert!((left_at(width as f32 + 1.0) - left_at(width as f32)).abs() <= 1.0);
        }
        assert_eq!(
            resolve_chat_canvas_lane(1024.0, 48.0, 736.0, Some(732.0)),
            ChatLane {
                left: 48.0,
                width: 652.0,
                inset_end: 276.0
            }
        );
        assert_eq!(
            resolve_chat_canvas_lane(1024.0, 48.0, 736.0, None),
            ChatLane {
                left: 144.0,
                width: 736.0,
                inset_end: 0.0
            }
        );
        assert_eq!(
            resolve_chat_canvas_lane(390.0, 12.0, 736.0, None),
            ChatLane {
                left: 12.0,
                width: 366.0,
                inset_end: 0.0
            }
        );
    }
}
