from ..templates import template
import reflex as rx
import reflex_chakra as rc
from ..popups.settingImagePopup import settingImagePopup
from ..states import photoAcquisition_state
from ..photogrammetry.widgets import *
from ..states.photoAcquisition_state import PAState
from ..popups.focusStackCalibrationPopup import focus_stack_popup
from ..popups.config_popups import *
from ..components.liveView import liveView
from ..popups.thumbnailCapturePopup import thumbnail_capture_popup
from ..popups.cameraControlPopup import camera_control_popup
from ..components.thumbnail import *



@template(route="/photoAcquisition", title="Photo Acquisition", on_load=[PAState.setup_state, user_state.load_data])
def photoAcquisition() -> rx.Component:
    return rx.hstack(
        rx.vstack(
            rx.hstack(
                rx.spacer(),
                rx.card(
                    rx.hstack(
                        rx.icon("camera"),
                        rx.heading("Camera", size="4"),
                        rx.cond(
                            PAState.camera_connected,
                            rx.tooltip(
                                
                                rx.button(rx.icon('circle-power'),      
                                    on_click=PAState.toggle_camera(),
                                    color_scheme='green',
                                    loading=PAState.camera_connecting
                                ),
                                content = "Disconnect Camera",
                            ),
                            rx.tooltip(
                                rx.button(rx.icon('circle-power'),      
                                    on_click=PAState.toggle_camera(),
                                    color_scheme='red',
                                    loading=PAState.camera_connecting
                                ),
                                content = "Connect Camera",
                            )
                        ),
                        camera_config_popup(),
                        align="center",
                    ),
                ),
                rx.spacer(),
                rx.card(
                    rx.hstack(
                        rx.icon("database-backup"),
                        rx.heading("Turntable", size="4"),
                        rx.cond(
                            PAState.turntable_connected,
                            rx.tooltip(
                                rx.button(rx.icon('circle-power'),      
                                    on_click=PAState.toggle_turntable(),
                                    color_scheme='green',
                                    loading=PAState.turntable_connecting
                                ),
                                content = "Disconnect Turntable"
                            ),
                            rx.tooltip(
                                rx.button(rx.icon('circle-power'),      
                                    on_click=PAState.toggle_turntable(),
                                    color_scheme='red',
                                    loading=PAState.turntable_connecting
                                ),
                                content= "Connect Turntable"
                            ),
                        ),
                        turntable_config_popup(),

                        align="center",
                    ),
                ),
                rx.spacer(),
                rx.card(
                    rx.hstack(
                        rx.icon("biceps-flexed"),
                        rx.heading("Arm", size="4"),
                        rx.cond(
                            PAState.arm_connected,
                            rx.tooltip(
                                rx.button(rx.icon('circle-power'),      
                                    #on_click=PAState.toggle_turntable(),
                                    color_scheme='green',
                                    loading=PAState.arm_connecting,
                                ),
                                content = "Disconnect Arm",
                            ),
                            rx.tooltip(
                                rx.button(rx.icon('circle-power'),      
                                    #on_click=PAState.toggle_turntable(),
                                    color_scheme='red',
                                    loading=PAState.arm_connecting,
                                    disabled=True,
                                ),
                                content= "Connect Arm (Future update)"
                            ),
                        ),
                        arm_config_popup(),

                        align="center",
                    ),
                ),
                rx.spacer(),
                width="100%",
            ),

            rx.cond(
                PAState.camera_connected,
                rx.card(
                    rx.hstack(
                        rx.text("Battery", size="4"),
                        rx.progress(
                                    value=PAState.camera_battery_level,
                                    height="19px",
                                    color_scheme='green',
                                    width="80px",
                        ),
                        rx.text(f"{PAState.camera_battery_level}%", size="3"),
                        rx.text("Cards: ", size="3"),
                        rx.cond(
                            PAState.camera_card_present_1,
                            rx.hstack(
                                rx.text(f"1 ({PAState.camera_card_capacity_1}GB)", size="3"),
                                rx.progress(
                                            value=PAState.camera_card_usage_1,
                                            height="19px",
                                            color_scheme='green',
                                            width="80px",
                                ),
                                rx.text(f"{PAState.camera_card_usage_1}%", size="3"),
                                rx.spacer()
                            ), None
                        ),
                        rx.cond(
                            PAState.camera_card_present_2,
                            rx.hstack(
                                rx.text(f"2 ({PAState.camera_card_capacity_2}GB)", size="3"),
                                rx.progress(
                                            value=PAState.camera_card_usage_2,
                                            height="19px",
                                            color_scheme='green',
                                            width="80px",
                                ),
                                rx.text(f"{PAState.camera_card_usage_2}%", size="3"),
                                rx.spacer()
                            ), None
                        ),
                        rx.cond(
                            ~(PAState.camera_card_present_1 | PAState.camera_card_present_2),
                            rx.text("No Memory Cards Present", size="4")
                        )
                    ),
                ),
            ),
            rx.card(
                rx.vstack(
                    subject_heading("Select Current Model and Sequence", "box"),
                    rx.hstack(
                        rx.vstack(
                            option_input("Current Model", PAState.selected_mesh_model_string, PAState.mesh_model_list, PAState.set_mesh_model, width="500px")
                        ),
                        rx.vstack(
                            rx.heading("Actions", size="3"),
                            rx.hstack(
                                new_item_dialog("New Model", "Create a new 3D Model", state=PAState, value="model_form_name", action=PAState.new_mesh_model, icon='plus'),
                                edit_current_model_popup(),
                                #new_item_dialog("Edit Model", "Edit a 3D Model", state=PAState, value="selected_mesh_model_name", action=PAState.edit_mesh_model, icon='pencil'),
                                
                            )
                        ),
                        align="center",
                    ),
                    rx.hstack(
                        option_input("Orientation", PAState.orientation_string, ["1","2","3","4","5","6","7","8","9"], PAState.set_orientation, width="85px"),
                        option_input("Current Sequence", PAState.selected_photo_sequence_string, PAState.photo_sequences_list, PAState.set_photo_sequence, width="400px"),
                        rx.vstack(
                            rx.heading("Actions", size="3"),
                            rx.hstack(
                                new_item_dialog("New Sequence", "Create a new photo sequence", state=PAState, value="photo_sequence_form_name", action=PAState.new_photo_sequence, icon='plus'),
                                edit_current_sequence_popup(),
                                #new_item_dialog("Edit Sequence", "Edit photo sequence", state=PAState, value="photo_sequence_form_name", action=PAState.edit_photo_sequence, icon='pencil'),
                                # rx.tooltip(rx.button(rx.icon('pencil'), on_click=PAState.edit_photo_sequence), content="Edit Sequence"),
                            ),
                        ),
                    ),
                ),
                
                width="100%",
                spacing="4",
                justify="between",
                flex_direction=["column", "column", "row"],
            ),
            rx.cond(
                PAState.camera_connected,
                rx.hstack(
                    rx.spacer(),
                    camera_control_popup(),
                    rx.spacer(),
                    rx.button(
                        "Start Capture of Photo Sequence",
                        width="45%",
                        disabled=~(PAState.camera_connected & PAState.turntable_connected),
                        on_click=PAState.start_photo_sequence,
                    ),
                    rx.spacer(),
                    width="100%",
                ),
            ),
            rx.card(
                rx.vstack(
                    subject_heading(f"Photo Sequence Settings for:  {PAState.selected_photo_sequence_string}", "camera"),
                    rx.hstack(
                        rx.heading("Camera Presets", size="3"),
                        rx.select(
                            PAState.camera_presets_list,
                            size="2",
                            value=PAState.selected_camera_preset_string,
                            on_change=PAState.set_camera_preset,
                            width="150px"
                        ),
                        new_item_dialog("New Camera Preset", "Creates a new camera preset for aperture, iso, shutter speed", state=PAState, value="camera_preset_form_name", action=PAState.new_camera_preset, icon='plus'),
                        rx.tooltip(
                            rx.flex(
                                rx.button(rx.icon('camera'), 
                                    on_click=PAState.take_picture(), 
                                    disabled=~PAState.camera_connected
                                ),
                            ),
                            content="Take Full Res Photo With These Settings",
                        ),
                        align="center"
                    ),
                    rx.hstack(
                        option_input("Aperture", PAState.aperture, PAState.aperture_values, PAState.set_aperture, width="100px"),
                        option_input("ISO", PAState.iso, PAState.iso_values, PAState.set_iso, width="100px"),
                        option_input("Shutter", PAState.shutter_speed, PAState.shutter_speeds, PAState.set_shutter_speed, width="100px"),
                        rx.vstack(
                            rx.heading("_", size="3"),
                            rx.button(rx.icon("refresh-ccw"), on_click=PAState.set_to_auto_shutter_speed(), disabled=~PAState.camera_connected, loading=PAState.checking_auto_shutter_speed)
                        ),
                    ),
                    rx.hstack(
                        rx.heading("Turntable", size="3"),
                        rx.divider(orientation="vertical", size="2"),
                        rx.heading("Total Range: ", size="3"),
                        rx.input(
                            # placeholder="7.1",
                            value=PAState.rotation_total,
                            on_change=PAState.set_rotation_total(),
                            width='50px'
                        ),
                        rx.heading("Step Size: ", size="3"),
                        rx.input(
                            # placeholder="7.1",
                            value=PAState.rotation_step,
                            on_change=PAState.set_rotation_step(),
                            width='50px'
                        ),
                        rx.tooltip(rx.flex(rx.button(rx.icon("sliders-horizontal"), on_click=PAState.toggle_show_turntable_controls, disabled=~PAState.turntable_connected),), content="Toggle Show Turntable Controls"),
                        rx.cond(
                            PAState.show_turntable_controls,
                            rx.hstack(
                                rx.divider(orientation="vertical", size="2"),
                                rx.tooltip(rx.flex(rx.button(rx.icon('rotate-ccw'), on_click=PAState.rotate_ccw, size="2", disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Rotate Counter Clockwise"),
                                rx.tooltip(rx.flex(rx.button(rx.icon('rotate-cw'), on_click=PAState.rotate_cw, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Rotate Clockwise"),
                                rx.tooltip(rx.flex(rx.button(rx.icon('home'), on_click=PAState.rotate_home, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Go Home"),
                                rx.tooltip(rx.flex(rx.button(rx.icon('arrow-down-to-dot'), on_click=PAState.set_turntable_origin, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),),content="Set Home"),
                            ),
                        ),
                        align = "center",  
                    ),
                    rx.vstack(
                        rx.hstack(
                            rx.checkbox(
                                size="3",
                                checked=PAState.hdr,
                                on_change=PAState.toggle_hdr(),
                                margin_top="3px"
                            ),
                            rx.text("HDR", size="3", weight="bold", margin_top="3px"),
                            rx.cond(
                                PAState.hdr,
                                rx.hstack(
                                    rx.divider(orientation="vertical", size="2"),
                                    rx.text("Exposures", size="3", weight="bold", margin_top ="3px"),
                                    rx.input(
                                        value=PAState.hdr_exposures,
                                        on_change=PAState.set_hdr_exposures,
                                        width='30px'
                                    ),
                                    rx.text("Step", size="3", weight="bold", margin_top="3px"),
                                    rx.select(
                                        PAState.hdr_step_sizes,
                                        size="2",
                                        value=PAState.hdr_step_size,
                                        on_change=PAState.set_hdr_step_size,
                                        width="62px"
                                    ),
                                ),
                                
                            ),
                        ),
                        rx.hstack(
                            rx.checkbox(
                                size="3",
                                checked=PAState.focus_stacking,
                                on_change=PAState.toggle_focus_stacking(),
                                margin_top="3px"
                            ),
                            rx.text("Focus Stacking", size="3", weight="bold", margin_top="3px"),
                            rx.cond(
                                PAState.focus_stacking,
                                rx.hstack(
                                    rx.divider(orientation="vertical", size="2"),
                                    rx.text("Shots", size="3", margin_top="3px", weight="bold"),
                                    rx.input(
                                        value=PAState.focus_steps,
                                        on_change=PAState.set_focus_steps,
                                        width='40px'
                                    ),
                                    rx.text("Width", size="3", margin_top="3px", weight="bold"), 
                                    rx.input(
                                        value=PAState.focus_step_width,
                                        on_change=PAState.set_focus_step_width(),
                                        width='40px'
                                    ),
                                    focus_stack_popup(),
                                    #settingImagePopup("focus_step", PAState.default_test_focus_step_values, "Take a series of photos with different focus points", icon_name='layout-grid'),
                                    #rx.tooltip(rx.button(rx.icon('layout-grid'), on_click=PAState.take_picture(), disabled=~PAState.camera_connected), content="Take Set of Focus Stacking Shots for Tuning"),
                                )
                            ),
                        ),
                    ),
                    
                    width="100%"
                ),
                
                width="100%",
                spacing="4",
                flex_direction=["column", "column", "row"],
            ),
            spacing="6",
            width="500%",
            max_width="800px",
        ),
        rx.flex(
            rx.divider(orientation="vertical", size="4"),
            direction="column",
            width="3",
            height="100%",
        ),
        rx.card(
            rx.vstack(
                rx.flex(
                    rx.tabs.root(
                        rx.tabs.list(
                            rx.tabs.trigger("Model Notes", value="tab1"),
                            rx.tabs.trigger("Live Preview", value="tab2"),
                            rx.tabs.trigger("Camera Settings", value="tab3")
                        ),
                        rx.tabs.content(
                            rx.vstack(
                                rx.cond(
                                    PAState.selected_mesh_model != None,
                                    thumbnail_image_with_capture(PAState),
                                    rx.text("No model ID available"),
                                ),
                                # rx.cond(
                                #     PAState.thumbnail_exists,
                                #     rx.image(src=PAState.thumbnail_location_string, width="100%", height="auto", flex_grow="1", flex_shrink="1"),
                                #     rx.box(
                                #         rx.upload(
                                #             rx.vstack(
                                #                 rx.text("Drag and Drop Image"),
                                #                 rx.text("--- Or ---", padding_top="6px", padding_bottom="6px"),
                                #                 rx.button("Select Image"),
                                #                 align="center"
                                #             ),
                                #             id="upload1",
                                #             max_files=1,  # Limit to single file
                                #             accept={
                                #                 "image/jpeg": [".jpg", ".jpeg"],
                                #             },
                                #             on_drop=PAState.save_thumbnail(rx.upload_files(upload_id="upload1")),
                                #         ),
                                #         rx.hstack(
                                #             rx.spacer(),
                                #             rx.text("--- Or ---"),
                                #             rx.spacer(),
                                #             padding_top = "6px",
                                #             width = "100%",
                                #         ),
                                #         rx.hstack(
                                #             rx.spacer(),
                                #             thumbnail_capture_popup(),
                                #             rx.spacer(),
                                #             padding_top = "6px",
                                #             width = "100%",
                                #         ),
                                #         width = "100%",
                                #     ),
                                # ),
                                rx.divider(),
                                editable_text(value=PAState.selected_model_description, on_change=PAState.update_model_description, width="100%"),
                                rx.text_area(placeholder="Model Notes...", size="3", rows="25", value=PAState.selected_model_notes, on_change=PAState.update_model_notes, resize="both", flex_grow="1", flex_shrink="1", width="100%", debounce_timeout=1000),
                                flex_grow="1",
                                flex_shrink="1",
                                width="100%",
                            ),
                            value="tab1",
                        ),
                        rx.tabs.content(
                            rx.flex(
                                liveView(),
                                width="100%",
                            ),
                            
                            value="tab2",
                        ),
                        rx.tabs.content(
                            rx.vstack(
                                rx.hstack(
                                    rx.button(rx.icon('refresh-ccw'), on_click=PAState.refresh_camera_info),
                                    rx.cond(
                                        ~PAState.camera_connected,
                                        rx.text("Connect camera to see more info"),
                                    ),
                                    align="center",
                                ),
                                rx.cond(
                                    PAState.camera_connected,
                                    rx.html(PAState.camera_settings)
                                ),

                            ),
                            value="tab3",
                        ),
                        width="600px",
                        flex_grow="1",
                        default_value="tab1",
                    ),
                    spacing="6",
                    width="100%",
                    flex_grow="1"
                )
            ),
        ),
    )


""" rx.hstack(
                                option_input("Aperture", PAState.camera_aperture, PAState.aperture_values, PAState.set_camera_aperture, width="100px"),
                                option_input("ISO", PAState.camera_iso, PAState.iso_values, PAState.set_camera_iso, width="100px"),
                                option_input("Shutter", PAState.camera_shutter_speed, PAState.shutter_speeds, PAState.set_camera_shutter_speed, width="100px"),
                            ) 
rx.vstack(
                                rx.heading("Actions", size="3"),
                                rx.hstack(
                                    rx.tooltip(rx.flex(rx.button(rx.icon('rotate-ccw'), on_click=PAState.rotate_ccw, size="2", disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Rotate Counter Clockwise"),
                                    rx.tooltip(rx.flex(rx.button(rx.icon('rotate-cw'), on_click=PAState.rotate_cw, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Rotate Clockwise"),
                                    rx.tooltip(rx.flex(rx.button(rx.icon('home'), on_click=PAState.rotate_home, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),), content="Go Home"),
                                    rx.tooltip(rx.flex(rx.button(rx.icon('arrow-down-to-dot'), on_click=PAState.set_turntable_origin, size="2",disabled=~PAState.turntable_connected | PAState.turntable_moving),),content="Set Home"),
                                )
                            )                  
                            
                            
                            
                            
                            """
