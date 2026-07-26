use gphoto2::{widget::Widget, Context};

fn main() {
    let context = Context::new().unwrap();
    let camera = context.autodetect_camera().wait().unwrap(); 
    // Intentionally use methods that might fail to see suggestions
    let config = camera.config().wait().unwrap();
    
    // Test get_child_by_name vs mut
    let child = config.get_child_by_name("manualfocusdrive");
    // let child_mut = config.get_child_by_name_mut("manualfocusdrive"); // Uncomment to test

    if let Ok(widget) = child {
        match widget {
            Widget::Radio(radio) => {
                // Compiler suggested 'choice', so maybe 'set_choice'?
                radio.set_choice("test");
            },
            Widget::Toggle(toggle) => {
                toggle.set_toggled(true);
            },
            Widget::Range(range) => {
                range.set_value(1.0);
            },
            Widget::Text(text) => {
                text.set_value("foo");
            },
            _ => {}
        }
    }
}
