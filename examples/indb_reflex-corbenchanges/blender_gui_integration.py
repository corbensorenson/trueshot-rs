import bpy
import os
import subprocess
import json
import logging
from bpy.props import StringProperty, EnumProperty, BoolProperty

# Dynamic paths for portability
PROJECT_ROOT = os.path.dirname(os.path.abspath(__file__))
FETCH_SCRIPT = os.path.join(PROJECT_ROOT, "fetch_models.py")
MAIN_SCRIPT = os.path.join(PROJECT_ROOT, "Corbens_pipeline", "main.py")
EXTERNAL_PYTHON = "python"  # Update to venv path once imports are fixed
STATUS_FILE = os.path.join(PROJECT_ROOT, "sd_card_status.json")

# Function to fetch models from the external script and cache them
def fetch_models():
    try:
        result = subprocess.run(
            [EXTERNAL_PYTHON, FETCH_SCRIPT],
            capture_output=True,
            text=True,
            check=True,
            cwd=PROJECT_ROOT
        )
        stdout_content = result.stdout.strip()
        try:
            models_data = json.loads(stdout_content)
        except json.JSONDecodeError:
            json_start = stdout_content.find('[')
            if json_start != -1:
                json_str = stdout_content[json_start:]
                models_data = json.loads(json_str)
            else:
                raise
        
        bpy.context.scene.cached_models = json.dumps(models_data)
        print(f"Fetched and cached {len(models_data)} models")
        return models_data
    except subprocess.CalledProcessError as e:
        print(f"Error running fetch_models.py: {e}")
        print(f"stderr: {e.stderr}")
        return []
    except json.JSONDecodeError as e:
        print(f"Error parsing JSON from fetch_models.py: {e}")
        print(f"Raw output: '{result.stdout}'")
        return []

# Timer callback to check SD card status
def check_sd_status(context):
    if os.path.exists(STATUS_FILE):
        with open(STATUS_FILE, "r") as f:
            status = json.load(f)
        if status.get("safe_to_remove", False):
            bpy.ops.wm.message_box(message="Safe to remove SD card.")
            os.remove(STATUS_FILE)  # Clean up
            return None  # Stop timer
    return 1.0  # Check every second

# Operator to refresh models
class RefreshModelsOperator(bpy.types.Operator):
    bl_idname = "fileselect.refresh_models"
    bl_label = "Refresh Models"
    bl_description = "Fetch the latest model data from the database"

    def execute(self, context):
        fetch_models()
        return {'FINISHED'}

# Operator for selecting a folder
class FILESELECT_OT_select_folder(bpy.types.Operator):
    bl_idname = "fileselect.select_folder"
    bl_label = "Select a Folder"
    bl_description = "Open file explorer to select a folder"

    folderpath: StringProperty(subtype='DIR_PATH', default="")

    def execute(self, context):
        context.scene.session_dir = self.folderpath
        print(f"Selected folder: {self.folderpath}")
        return {'FINISHED'}

    def invoke(self, context, event):
        context.window_manager.fileselect_add(self)
        return {'RUNNING_MODAL'}

# Operator for extracting photos and running the pipeline
class ExtractPhotosOperator(bpy.types.Operator):
    bl_idname = "sdcard.extract_photos"
    bl_label = "Extract and Process Photos"
    bl_description = "Extract photos and process them with the selected model"

    def execute(self, context):
        if not context.scene.model_name or not context.scene.session_dir:
            self.report({'ERROR'}, "Please select a model and SD card directory first")
            return {'CANCELLED'}
        
        models_data = json.loads(context.scene.cached_models)
        selected_model = next((m for m in models_data if m["id"] == context.scene.model_name), None)
        if not selected_model:
            self.report({'ERROR'}, "Selected model not found in cached data")
            return {'CANCELLED'}
        
        # Initialize progress bar
        wm = context.window_manager
        wm.progress_begin(0, 100)

        model_json = json.dumps(selected_model)
        sd_dir = context.scene.session_dir
        clear_sd = context.scene.clear_sd_card
        backup_to_nas = context.scene.backup_to_nas
        cmd = [
            EXTERNAL_PYTHON,
            MAIN_SCRIPT,
            "--model", model_json,
            "--sd_dir", sd_dir,
            "--clear_sd" if clear_sd else "--no_clear_sd",
            "--backup_to_nas" if backup_to_nas else "--no_backup_to_nas"
        ]
        
        try:
            # Step 2: Extract and Group (progress: 0-20%)
            wm.progress_update(0)
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                universal_newlines=True
            )

            # Simulate progress updates for extraction and grouping
            for i in range(1, 21):
                line = process.stdout.readline()
                if not line and process.poll() is not None:
                    break
                wm.progress_update(i)
                context.window_manager.event_timer_add(0.1, window=context.window)

            process.wait()
            if process.returncode != 0:
                error = process.stderr.read()
                self.report({'ERROR'}, f"Pipeline failed: {error}")
                wm.progress_end()
                return {'CANCELLED'}

            stdout_content = process.stdout.read()
            print(f"Pipeline output: {stdout_content}")
            if process.stderr:
                stderr_content = process.stderr.read()
                print(f"Pipeline errors: {stderr_content}")

            output_data = json.loads(stdout_content.strip())
            if output_data.get("safe_to_remove", False):
                self.report({'INFO'}, "Pipeline completed. Safe to remove SD card.")
            else:
                self.report({'INFO'}, "Pipeline started. Waiting for SD card processing...")
                bpy.app.timers.register(lambda: check_sd_status(context), first_interval=1.0)

            # Simulate progress for remaining steps (20-100%)
            for i in range(21, 101):
                wm.progress_update(i)
                context.window_manager.event_timer_add(0.1, window=context.window)

        except subprocess.CalledProcessError as e:
            print(f"Pipeline failed: {e}")
            print(f"stderr: {e.stderr}")
            self.report({'ERROR'}, f"Pipeline failed: {e.stderr}")
            wm.progress_end()
            return {'CANCELLED'}
        except json.JSONDecodeError as e:
            print(f"Error parsing pipeline output: {e}")
            self.report({'ERROR'}, "Pipeline completed but output parsing failed")
            wm.progress_end()
            return {'CANCELLED'}
        finally:
            wm.progress_end()
        
        return {'FINISHED'}

# Operator for viewing mesh
class ViewMeshOperator(bpy.types.Operator):
    bl_idname = "sdcard.view_mesh"
    bl_label = "View Final Mesh"

    def execute(self, context):
        session_dir = context.scene.session_dir
        mesh_path = os.path.join(session_dir, "final.fbx")
        if not os.path.exists(mesh_path):
            self.report({'ERROR'}, f"Mesh file not found at {mesh_path}")
            return {'CANCELLED'}
        bpy.ops.import_scene.fbx(filepath=mesh_path)
        return {'FINISHED'}

# Operator for selecting a model with popup
class SelectModelOperator(bpy.types.Operator):
    bl_idname = "fileselect.select_model"
    bl_label = "Select Model"
    bl_description = "Select a model from the database"

    search_query: StringProperty(
        name="Search",
        default="",
        description="Search models by name or ID"
    )

    def model_items(self, context):
        if not hasattr(context.scene, 'cached_models') or not context.scene.cached_models:
            models_data = fetch_models()
        else:
            models_data = json.loads(context.scene.cached_models)
        
        if not models_data:
            print("No models available, using empty list")
            return [("NONE", "No matches", "No models available")]

        filtered_models = []
        for model in models_data:
            if (self.search_query.lower() in model["name"].lower() or 
                self.search_query in model["id"]):
                filtered_models.append((model["id"], model["name"], f"ID: {model['id']}"))
        if not filtered_models:
            filtered_models.append(("NONE", "No matches", "No models match the search"))
        return filtered_models

    selected_model: EnumProperty(
        name="Models",
        description="Choose a model",
        items=model_items,
        default=None
    )

    def execute(self, context):
        if self.selected_model != "NONE":
            context.scene.model_name = self.selected_model
            models_data = json.loads(context.scene.cached_models)
            for model in models_data:
                if model["id"] == self.selected_model:
                    context.scene.model_name_display = model["name"]
                    print(f"Selected model: {model['name']} (ID: {self.selected_model})")
                    context.scene.photo_sequences = json.dumps(model["photo_sequences"])
                    break
        return {'FINISHED'}

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=400)

    def draw(self, context):
        layout = self.layout
        layout.prop(self, "search_query", text="Search")
        layout.prop(self, "selected_model", text="Model")

# Panel
class SDCardToolsPanel(bpy.types.Panel):
    bl_label = "SD Card Extraction"
    bl_idname = "VIEW3D_PT_sd_card_tools"
    bl_space_type = 'VIEW_3D'
    bl_region_type = 'UI'
    bl_category = "3D Pipeline"

    def draw(self, context):
        layout = self.layout
        layout.operator("fileselect.refresh_models", text="Refresh Models")
        layout.operator("fileselect.select_model", text="Select Model")
        layout.prop(context.scene, "model_name_display", text="Model")
        layout.operator("fileselect.select_folder", text="Select SD Photo Directory")
        layout.prop(context.scene, "session_dir", text="SD Dir")
        layout.prop(context.scene, "backup_to_nas", text="Backup to NAS")
        layout.prop(context.scene, "clear_sd_card", text="Clear SD Card After Copy")
        layout.operator("sdcard.extract_photos", text="Extract and Process Photos")

def register():
    bpy.utils.register_class(SDCardToolsPanel)
    bpy.utils.register_class(ExtractPhotosOperator)
    bpy.utils.register_class(ViewMeshOperator)
    bpy.utils.register_class(FILESELECT_OT_select_folder)
    bpy.utils.register_class(SelectModelOperator)
    bpy.utils.register_class(RefreshModelsOperator)

    bpy.types.Scene.session_dir = StringProperty(name="Session Directory", default="No folder selected")
    bpy.types.Scene.model_name = StringProperty(name="Model ID", default="No model selected")
    bpy.types.Scene.model_name_display = StringProperty(name="Model Name", default="No model selected")
    bpy.types.Scene.photo_sequences = StringProperty(name="Photo Sequences", default="[]")
    bpy.types.Scene.cached_models = StringProperty(name="Cached Models", default="")
    bpy.types.Scene.clear_sd_card = BoolProperty(
        name="Clear SD Card After Copy",
        description="Delete SD card contents after extraction and backup",
        default=False
    )
    bpy.types.Scene.backup_to_nas = BoolProperty(
        name="Backup to NAS",
        description="Backup extracted photos to NAS",
        default=True
    )

def unregister():
    bpy.utils.unregister_class(SDCardToolsPanel)
    bpy.utils.unregister_class(ExtractPhotosOperator)
    bpy.utils.unregister_class(ViewMeshOperator)
    bpy.utils.unregister_class(FILESELECT_OT_select_folder)
    bpy.utils.unregister_class(SelectModelOperator)
    bpy.utils.unregister_class(RefreshModelsOperator)

    del bpy.types.Scene.session_dir
    del bpy.types.Scene.model_name
    del bpy.types.Scene.model_name_display
    del bpy.types.Scene.photo_sequences
    del bpy.types.Scene.cached_models
    del bpy.types.Scene.clear_sd_card
    del bpy.types.Scene.backup_to_nas

def integrate_with_gui(refined_mesh):
    logging.info("Integrating with Blender GUI...")
    register()

if __name__ == "__main__":
    register()