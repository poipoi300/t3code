use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePreview<T> {
    pub images: Vec<T>,
    pub index: usize,
}

pub fn work_group_image_gallery<T: Clone>(
    viewed_images: &[(String, Option<T>)],
    clicked_path: Option<&str>,
    preview: Rc<ImagePreview<T>>,
) -> Rc<ImagePreview<T>> {
    let Some(selected) = preview
        .images
        .get(preview.index)
        .filter(|_| preview.images.len() == 1)
    else {
        return preview;
    };
    let mut images = Vec::new();
    let mut index = None;
    for (path, item) in viewed_images {
        if Some(path.as_str()) == clicked_path {
            index = Some(images.len());
            images.push(selected.clone());
        } else if let Some(item) = item {
            images.push(item.clone());
        }
    }
    index.map_or_else(
        || preview.clone(),
        |index| Rc::new(ImagePreview { images, index }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pages_clicked_image_through_resolved_group_images_preserving_its_action_source() {
        let first = json!({"src":"https://assets.test/first.png","name":"first.png"});
        let second = json!({"src":"https://assets.test/second.png","name":"second.png"});
        let third = json!({"src":"https://assets.test/third.png","name":"third.png"});
        let clicked =
            json!({"src":"https://assets.test/second.png#fresh","actionsSource":{"token":"fresh"}});
        let viewed = vec![
            ("/first".into(), Some(first.clone())),
            ("/pending".into(), None),
            ("/second".into(), Some(second)),
            ("/third".into(), Some(third.clone())),
        ];
        let single = Rc::new(ImagePreview {
            images: vec![clicked.clone()],
            index: 0,
        });
        let gallery = work_group_image_gallery(&viewed, Some("/second"), single.clone());
        assert_eq!(
            *gallery,
            ImagePreview {
                images: vec![first, clicked, third],
                index: 1
            }
        );
        assert!(Rc::ptr_eq(
            &single,
            &work_group_image_gallery(&viewed, Some("/other"), single.clone())
        ));
        assert!(Rc::ptr_eq(
            &single,
            &work_group_image_gallery(&viewed, None, single.clone())
        ));
        assert!(Rc::ptr_eq(
            &gallery,
            &work_group_image_gallery(&viewed, Some("/second"), gallery.clone())
        ));
        let invalid = Rc::new(ImagePreview {
            images: vec![],
            index: 0,
        });
        assert!(Rc::ptr_eq(
            &invalid,
            &work_group_image_gallery(&viewed, Some("/second"), invalid.clone())
        ));
    }
}
