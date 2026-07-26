from .exposure_fusion import ExposureFusion

class hdrFusion():
    def __init__(self, perform_alignment = False):
        self.fuser = ExposureFusion(perform_alignment = perform_alignment)

    def combine(self, images):
        return self.fuser(images)
    



    
