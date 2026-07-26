import gphoto2 as gp

class Cannon:
    def __init__(self, aperture=None, shutter_speed=None, iso=None, shots=None, manual_refocus=False):
        self.aperture = aperture
        self.shutter_speed = shutter_speed
        self.iso = iso
        self.shots = shots

    def set_aperture(self, aperture):
        self.aperture = aperture

    def set_shutter_speed(self, shutter_speed):
        self.shutter_speed = shutter_speed

    def set_iso(self, iso):
        self.iso = iso

    def set_number_of_shots(self, shots):
        self.shots = shots

    def perform_focus_shifted_shot_sequence(self):
        pass