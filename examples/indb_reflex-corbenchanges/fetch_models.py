# fetch_models.py
import sys
import json
from pga.models.mesh_model import MeshModel

def main():
    # Redirect all stdout to stderr at the start
    original_stdout = sys.stdout
    sys.stdout = sys.stderr
    
    # Fetch all models from the database
    models = MeshModel.all()
    
    # Restore stdout just before printing JSON
    sys.stdout = original_stdout
    
    # Serialize to a list of dictionaries
    data = []
    for model in models:
        photo_sequences_data = [
            {
                "sequence_number": seq.sequence_number,
                "focus_steps": seq.focus_steps,
                "focus_step_width": seq.focus_step_width,
                "rotation_total": seq.rotation_total,
                "rotation_step": seq.rotation_step,
                "orientation": seq.orientation,
                "hdr_exposures": seq.hdr_exposures,
                "photos_start": seq.photos_start,
                "photos_end": seq.photos_end,
            }
            for seq in model.photo_sequences
        ]
        data.append({
            "id": str(model.id),
            "name": model.name,
            "description": model.description,
            "photo_sequences": photo_sequences_data
        })
    
    # Print only the JSON to stdout
    print(json.dumps(data), flush=True)

if __name__ == "__main__":
    main()