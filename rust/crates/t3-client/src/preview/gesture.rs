#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    North,
    Northeast,
    East,
    Southeast,
    South,
    Southwest,
    West,
    Northwest,
}
pub struct Gesture {
    pub pointer_id: u64,
    pub pointer_type: String,
    pub pointer: Position,
    pub frame: Frame,
    pub direction: Option<Direction>,
    pub source: Size,
    pub moved: bool,
}
pub struct Pointer {
    pub pointer_id: u64,
    pub buttons: u16,
    pub position: Position,
}
#[derive(Debug, PartialEq)]
pub enum Step {
    Ignore,
    End,
    Move(Position),
    Resize(Frame),
}

fn clamp(position: Position, container: Size, player: Size) -> Position {
    Position {
        x: position
            .x
            .max(12.)
            .min((container.width - player.width - 12.).max(12.)),
        y: position
            .y
            .max(12.)
            .min((container.height - player.height - 12.).max(12.)),
    }
}

fn resize(
    start: Frame,
    direction: Direction,
    delta: Position,
    source: Size,
    container: Size,
) -> Frame {
    use Direction::*;
    let west = matches!(direction, West | Northwest | Southwest);
    let east = matches!(direction, East | Northeast | Southeast);
    let north = matches!(direction, North | Northwest | Northeast);
    let south = matches!(direction, South | Southwest | Southeast);
    let right = start.x + start.width;
    let bottom = start.y + start.height;
    let max_width = if west {
        right - 12.
    } else if east {
        container.width - 12. - start.x
    } else {
        container.width - 24.
    };
    let max_height = if north {
        bottom - 12.
    } else if south {
        container.height - 12. - start.y
    } else {
        container.height - 24.
    };
    let desired_width = start.width
        + if east {
            delta.x
        } else if west {
            -delta.x
        } else {
            0.
        };
    let desired_height = start.height
        + if south {
            delta.y
        } else if north {
            -delta.y
        } else {
            0.
        };
    let aspect = source.width / source.height;
    let desired = if (east || west) && (north || south) {
        (desired_width + desired_height / aspect) / (1. + 1. / aspect.powi(2))
    } else if east || west {
        desired_width
    } else {
        desired_height * aspect
    };
    let minimum = start
        .width
        .min(240f64.max(150. * source.width / source.height));
    let width = desired
        .max(minimum)
        .min(max_width.max(1.))
        .min((max_height * aspect).max(1.));
    let size = Size {
        width: width.round(),
        height: (width / aspect).round(),
    };
    let position = clamp(
        Position {
            x: if west { right - size.width } else { start.x },
            y: if north { bottom - size.height } else { start.y },
        },
        container,
        size,
    );
    Frame {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

pub fn step(gesture: &Gesture, pointer: Pointer, container: Size) -> Step {
    if pointer.pointer_id != gesture.pointer_id {
        return Step::Ignore;
    }
    if pointer.buttons & 1 == 0 {
        return Step::End;
    }
    let delta = Position {
        x: pointer.position.x - gesture.pointer.x,
        y: pointer.position.y - gesture.pointer.y,
    };
    if !gesture.moved && delta.x.hypot(delta.y) < 6. {
        return Step::Ignore;
    }
    match gesture.direction {
        Some(direction) => Step::Resize(resize(
            gesture.frame,
            direction,
            delta,
            gesture.source,
            container,
        )),
        None => Step::Move(clamp(
            Position {
                x: gesture.frame.x + delta.x,
                y: gesture.frame.y + delta.y,
            },
            container,
            Size {
                width: gesture.frame.width,
                height: gesture.frame.height,
            },
        )),
    }
}
pub fn settle_resize(
    frame: Frame,
    direction: Direction,
    held: Size,
    source: Size,
    container: Size,
) -> Option<Frame> {
    (held.width * source.height != source.width * held.height).then(|| {
        resize(
            frame,
            direction,
            Position { x: 0., y: 0. },
            source,
            container,
        )
    })
}
