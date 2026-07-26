# gphoto2 implementation... with direct control through USB tethering
import gphoto2 as gp
import time
import asyncio

class Nikon:
    def __init__(self, aperture=None, shutter_speed=None, iso=None, 
                 focus_mode='Manual', storage='Card'):
        self.aperture = aperture
        self.shutter_speed = shutter_speed
        self.iso = iso
        # self.manual_refocus = manual_refocus
        self.storage=storage
        self.focus_mode = focus_mode
        # How much focus increment per step
        self.focus_multiplier = 30
        self.focus_position='near'
        self.initialize_camera()
        self.shutter_speeds = []
        self.connected = False

    
    async def initialize_camera(self):
        self.camera = gp.Camera()
        print(f"Camera is {self.camera}")
        
        detected = False
        i=0
        while i < 10:
            if self.connect_to_camera():
                detected = True
                break
            if(i>0):
                print(".", end="", flush=True)
            else:
                print("Camera not detected, retrying", end="", flush=True)
            i+=1
            time.sleep(1)
        if not detected:
            print("No camera detected")
            # raise Exception('Camera device not detected')
        else:
            self.connected = True
            self.shutter_speeds = [choice for choice in self.camera.get_config().get_child_by_label("Shutter Speed").get_choices()]

        
    def connect_to_camera(self):
        try:
            self.camera.init()
            return True
        except gp.GPhoto2Error as e:
            return False
        
    
    def gp_camera(self):
        return self.camera

    def perform_focus_shifted_shot_sequence(self, shots=14, focus_width=4, hdr=False, exposures=3):
        try:
            focus_step_widget = self.get_widget("Drive Nikon DSLR Manual focus")
            focus_steps = focus_width * self.focus_multiplier * (1 if self.focus_position=='near' else -1)


            for _ in range(shots):
                self.camera.capture(hdr, exposures)
                focus_step_widget.set_value(focus_steps)
                self.camera.set_config(config)

            self.focus_position='far' if self.focus_position=='near' else 'near'
        except gp.GPhoto2Error as e:
            print(f"Error starting focus shift shooting sequence: {e}")

        

    def get_widget(self,label):
        return self.get_widget_by_label(label)

    # Gets a widget by label, which can then get set to a value or subvalues can be accessed if present
    def get_widget_by_label(self, label):
        try:
            config = self.camera.get_config()
            return config.get_child_by_label(label)
        except gp.GPhoto2Error as e:
            print(f"Error getting widget: {e}")

    # Labels can reach parameters at all levles of nesting
    def set_parameter_by_label(self, p1, val=None):
        try:
            config = self.camera.get_config()
            property = config.get_child_by_label(p1)
            property.set_value(val)
            self.camera.set_config(config)
            return config
        except gp.GPhoto2Error as e:
            print(f"Error setting Capture Target: {e}")

    # Hopefully not needed....  must specify nesting parameters
    def set_parameter_by_name(self, p1,p2=None, val=None):
        try:
            config = self.camera.get_config()
            property = config.get_child_by_name(p1)
            if(p2):
                property = property.get_child_by_name(p2)
            property.set_value(val)
            self.camera.set_config(config)
            return config
        except gp.GPhoto2Error as e:
            print(f"Error setting Capture Target: {e}")

    
    def get_parameterl(self, p):
        return self.get_parameter_by_label(self, p)
    
    def get_parameter_by_label(self, p):
        try:
            config = self.camera.get_configp()
            property = config.get_child_by_label(p)
            return property.get_value()
        except gp.GPhoto2Error as e:
            print(f"Error getting parameterl: {e}")
        
    def capture(self, hdr=False, exposures=3):
        if(self.storage=='File'):
            pass
            # self.camera.capture(gp.GP_CAPTURE_IMAGE)
            # camera_file = self.camera.file_get(file_path.folder, file_path.name, gp.GP_FILE_TYPE_NORMAL)
            # camera_file.save('/path/to/save/image.jpg')
            # self.camera.exit()
        else:
            if(hdr):
                index=self.shutter_speeds[self.shutter_speed]

                # find in ss array and then move from 
                n=round(exposures/2)
                for i in range(index-n, index+n):
                    ss=self.shutter_speeds[i]
                    self.set_shutter_speed(ss)
                    self.camera.trigger_capture()
                
            else:
                self.camera.trigger_capture()

    # Card, Card1, Card2, RAM, File
    def set_capture_storage(self, storage='Card'):
        storage='Card1' if storage=='Card' else storage
        self.storage = storage

        if(storage=='Card1'):
            self.set_parameter_by_label("Capture Target", val='Memory card')
        if(storage=='Card2'):
            self.set_parameter_by_label("Capture Target", val='Memory card2')
        if(storage=='RAM'):
            self.set_parameter_by_label("Capture Target", val='Internal RAM')    
        elif(storage=='File'):
            pass

    # Options are Auto or Manual M, P, A, S
    def set_exposure_mode(self, value=True):
        self.set_parameter_by_label('Exposure Program', "M" if value=='Manual' else 'A')

    # Options are Manual or Auto
    def set_focus_mode(self, value = "On"):
        self.focus_mode=value
        val='On' if value=='Auto' else 'Manual'
        return self.set_parameter_by_label("Autofocus", val=value)
    
    def set_shutter_speed(self, value):
        self.shutter_speed=value
        self.set_parameter_by_label("Shutter Speed", val=value)
        self.shutter_speed=self.parameter("Shutter Speed")
        return self.shutter_speed
    
    def set_white_balance(self, value):
        self.white_balance=value
        return self.set_parameter_by_label("WhiteBalance", val=value)
    
    def set_aperture(self, aperture):
        self.aperture = aperture

    def set_iso(self, iso):
        self.iso = iso

    def get_current_battery_level(self):
        self.get_parameter("Battery Level")

    
    # Debugging info to sniff the parameters
    def get_settings(self):
        try:
            # Get the main configuration
            #only needs to be called once and it updates a internal widget list
            config = self.camera.get_config()
            

            # Print the main configuration settings
            for child in config.get_children():
                print(f"Label: {child.get_label()}")
                for sub_child in child.get_children():
                    print(f">{sub_child.get_label()}: {sub_child.get_value()}")
                    
        except gp.GPhoto2Error as e:
            print(f"Error getting main configuration settings: {e}")