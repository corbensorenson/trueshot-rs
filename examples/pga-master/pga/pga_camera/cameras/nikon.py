# gphoto2 implementation... with direct control through USB tethering
import gphoto2 as gp
import time
from time import sleep
import math
import os

class Nikon:
    max_z_focus = 0
    def __init__(self, aperture=None, shutter_speed=None, iso=None, 
                 focus_mode='Manual', storage='Card'):
        self.aperture = aperture
        self.shutter_speed = shutter_speed
        self.iso = iso
        # self.manual_refocus = manual_refocus
        self.storage=storage
        self.focus_mode = focus_mode
        
        # How much focus increment per step
        self.focus_multiplier = 1
        self.focus_position='near'
        # self.initialize()
        self.shutter_speeds = [
            "900", "720", "600", "480", "300", "240", "180", "120", "90",
            "60", "30", "25", "20", "15", "13", "10", "8", "6", "5", "4", 
            "3", "2.5", "2", "1.6", "1.3", "1", "1/1.3", "1/1.6", "1/2", 
            "1/2.5", "1/3", "1/4", "1/5", "1/6", "1/8", "1/10", "1/13", 
            "1/15", "1/20", "1/25", "1/30", "1/40", "1/50", "1/60", "1/80",
            "1/100", "1/125", "1/160", "1/200", "1/250", "1/320", "1/400", 
            "1/500", "1/640", "1/800", "1/1000", "1/1250", "1/1600", "1/2000", 
            "1/2500", "1/3200", "1/4000", "1/5000", "1/6400", "1/8000", "1/10000", 
            "1/13000", "1/16000", "1/20000", "1/26000", "1/32000"
        ]
        self.connected = False
    
    async def initialize(self):
        self.camera = gp.Camera()
        print(f"Camera is {self.camera}")

        if await self.connect_to_camera():
            self.connected = True
        else:
             print("No camera detected")

        # Synchronize camera time
        abilities = self.camera.get_abilities()
        # get configuration tree
        config = self.camera.get_config()
        # find the date/time setting config item and set it
        if self.set_datetime(config, abilities.model):
            # apply the changed config
            self.camera.set_config(config)
        else:
            print('Could not set date & time')

        self.set_capture_storage(self.storage)
        
    async def connect_to_camera(self):
        print(f"Connecting to camera: {self.camera}")
        try:
            self.camera.init()
            return True
        except gp.GPhoto2Error as e:
            return False
        
    def disconnect(self):
        self.camera.exit()
        self.connected = False

    def gp_camera(self):
        return self.camera
    
    # Capture a photo with current settings and download to file
    def capture_preview(self, file):
        cf=self.camera.capture_preview()
        cf.save(file)
        return True
    


    # Capture an image.  Specify hdr.  Uses currently set shutter speed and aperture
    def capture(self, hdr=False, exposures=3, hdr_step='1', config=None, file=None):
        print(f"Capturing image: hdr={hdr}, exposures={exposures}, hdr_step={hdr_step}")
        if(file != None):
            pass
            # file_path=self.camera.capture(gp.GP_CAPTURE_IMAGE)
            # print('Nikon: Captured image', file_path.folder, file_path.name)
            # camera_file = self.camera.file_get(file_path.folder, file_path.name, gp.GP_FILE_TYPE_NORMAL)
            # print(f"Camera file: {camera_file}")
            # target=os.path.join('assets', file_path.name)
            # print(f"Nikon: Saving image to {target}")
            # camera_file.save(file)
     
        else:
            if hdr:
                sb = self.shutter_speed
                # Assuming each shutter speed in the array is 1/3 stop apart
                st=int(round(eval(hdr_step)*3))
                print(f"Shutter speed: {sb}, HDR array step: {st}")
                #added a check here just in case
                try:
                    index = self.shutter_speeds.index(sb)
                except ValueError:
                    print(f"Shutter speed {sb} not found in shutter speeds list.")
                    return
                # find in ss array and then move from 
                n = math.floor(exposures/2)
                # for i in range(index-n, index+n+1):
                #     ss = self.shutter_speeds[i]
                #     self.set_shutter_speed(ss)
                #     self.camera.trigger_capture()
                
                # print(f"shutter speeds list: {self.shutter_speeds}")
                print(f"index = {index}")
                print(f"st = {st}")#i know its above i am tired of scrolling above shutterspeeds to find it
                for i in range(-n, n+1):
                    new_index = index + i * st
                    print(new_index)
                    #added a bounds check
                    if 0 <= new_index < len(self.shutter_speeds):
                        ss = self.shutter_speeds[new_index]
                        self.set_shutter_speed(ss, config)
                        print(f"Capturing image at shutter speed {ss}")
                        self.camera.trigger_capture()
                        # sleep(1)
                    else:
                        print(f"Index {new_index} out of range for shutter speeds list.")
                self.set_shutter_speed(sb,config)
            else:
                self.camera.trigger_capture()

        return None

    def perform_focus_shifted_shot_sequence(self, shots=14, focus_width=4, hdr=False, exposures=3, hdr_step='2', delay=0):
        print(f"Performing focus shifted shot sequence: shots={shots}, focus_width={focus_width}, hdr={hdr}, exposures={exposures}, hdr_step={hdr_step}")
        try:
            config = self.camera.get_config()
            # focus_step_widget = self.get_widget("Drive Nikon DSLR Manual focus")
            focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
            focus_steps = focus_width * self.focus_multiplier * (1 if self.focus_position=='near' else -1)
            print(f"Focus steps: {focus_steps}, starting from {self.focus_position}, offset={focus_step_widget.get_value()}")

            for _ in range(shots):
                print(f"Next Focus Step: offset={focus_step_widget.get_value()}")
                self.capture(hdr, exposures, hdr_step, config)
                sleep(delay)
                focus_step_widget.set_value((_+1)*focus_steps)
                # What was this for?
                self.camera.set_config(config)

            self.focus_position='far' if self.focus_position=='near' else 'near'
        except gp.GPhoto2Error as e:
            print(f"Error starting focus shift shooting sequence: {e}")

    
    def perform_focus_shift_test(self, shots=14, focus_width=4, savePath=None, delay=0.0):
        print(f"Performing focus shift test: shots={shots}, focus_width={focus_width}")
        try:
            config = self.camera.get_config()
            focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
            focus_steps = focus_width * self.focus_multiplier * (1 if self.focus_position=='near' else -1)
            print(f"Focus steps: {focus_steps}, starting from {self.focus_position}")
            baseSavePath = savePath
            for _ in range(shots):
                print(f"Next Focus Step: offset={focus_step_widget.get_value()}")
                image_path = f"{baseSavePath}/focus_test_{_}.jpg"
                self.capture_preview(image_path)
                sleep(delay)
                focus_step_widget.set_value((_+1)*focus_steps)
                # What was this for?
                self.camera.set_config(config)

            self.focus_position='far' if self.focus_position=='near' else 'near'
        except gp.GPhoto2Error as e:
            print(f"Error starting focus shift shooting sequence: {e}")

        

    # def get_widget(self,label):
    #     return self.get_widget_by_label(label)

    # # Gets a widget by label, which can then get set to a value or subvalues can be accessed if present
    # #
    # #  May not be valid if another config object is used...
    # def get_widget_by_label(self, label):
    #     try:
    #         config = self.camera.get_config()
    #         return config.get_child_by_label(label)
    #     except gp.GPhoto2Error as e:
    #         print(f"Error getting widget: {e}")

    # Labels can reach parameters at all levles of nesting
    def set_parameter_by_label(self, p1, val=None, config=None):
        try:
            config = config if config else self.camera.get_config()
            property = config.get_child_by_label(p1)
            property.set_value(val)
            self.camera.set_config(config)
            return config
        except gp.GPhoto2Error as e:
            print(f"Error setting Capture Target: {e}")

    # Hopefully not needed....  must specify nesting parameters
    def set_parameter_by_name(self, p1,p2=None, val=None, config=None):
        try:
            config = config if config else self.camera.get_config()
            property = config.get_child_by_name(p1)
            if(p2):
                property = property.get_child_by_name(p2)
            property.set_value(val)
            self.camera.set_config(config)
            return config
        except gp.GPhoto2Error as e:
            print(f"Error setting Capture Target: {e}")

    # def get_shutter_speeds(self):
    #         self.shutter_speeds = [choice for choice in self.camera.get_config().get_child_by_label("Shutter Speed").get_choices()]
    #         return self.shutter_speeds

    # def get_parameterl(self, p):
    #     return self.get_parameter_by_label(self, p)
    
    def get_parameter_by_label(self, p):
        try:
            config = self.camera.get_config()
            property = config.get_child_by_label(p)
            return property.get_value()
        except gp.GPhoto2Error as e:
            print(f"Error getting parameterl: {e}")
        

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

    # Options are Auto or Manual M, P, A, S, Auto
    def set_exposure_mode(self, value='Auto', config=None):
        self.set_parameter_by_label('Exposure Program', value, config=config)

    # Options are Manual or Auto
    def set_focus_mode(self, value = "Manual", config=None):
        self.focus_mode=value
        val='Off' if value=='Manual' else 'On'
        self.set_parameter_by_label("Autofocus", val=value, config=config)
    
        return self.set_parameter_by_label("Focus Mode", val="0" if value=='Manual' else "1", config=config)
    # Set Auto ISO to true of false - not yet implemented
    def set_auto_iso(self, value=True):
        pass
        # return self.set_parameter_by_label("Auto ISO", val=value)
    
    def get_shutter_speed(self):
        self.shutter_speed=self.get_parameter_by_label("Shutter Speed")
        print(f"Shutter speed of Nikon Implementation: {self.shutter_speed}")
        return self.shutter_speed
    
    def set_shutter_speed(self, value,config=None):
        self.shutter_speed=value
        self.set_parameter_by_label("Shutter Speed", val=str(value), config=config)
        # self.shutter_speed=self.get_parameter_by_label("Shutter Speed")
        return self.shutter_speed
    
    def set_exposure_compensation(self, value, config=None):
        self.white_balance=value
        return self.set_parameter_by_label("Exposure Compensation", val=value, config=config)
    
    def set_white_balance(self, value, config=None):
        self.white_balance=value
        return self.set_parameter_by_label("WhiteBalance", val=value, config=config)
    
    def set_aperture(self, aperture, config=None):
        self.aperture = aperture
        return self.set_parameter_by_label("F-Number", val=f"f/{aperture}", config=config)
        
    def set_iso(self, iso, config=None):
        self.iso = iso
        return self.set_parameter_by_label("ISO Speed", val=iso, config=config)
    
    def set_datetime(self, config, model):
        if model == 'Canon EOS 100D':
            OK, date_config = gp.gp_widget_get_child_by_name(config, 'datetimeutc')
            if OK >= gp.GP_OK:
                now = int(time.time())
                date_config.set_value(now)
                return True
        OK, sync_config = gp.gp_widget_get_child_by_name(config, 'syncdatetime')
        if OK >= gp.GP_OK:
            sync_config.set_value(1)
            return True
        OK, date_config = gp.gp_widget_get_child_by_name(config, 'datetime')
        if OK >= gp.GP_OK:
            widget_type = date_config.get_type()
            if widget_type == gp.GP_WIDGET_DATE:
                now = int(time.time())
                date_config.set_value(now)
            else:
                now = time.strftime('%Y-%m-%d %H:%M:%S')
                date_config.set_value(now)
            return True
        return False


    def get_current_battery_level(self):
        return self.get_parameter_by_label("Battery Level")

    def get_info(self, name=False):
        try:
            config = self.camera.get_config()
            s={}
            for child in config.get_children():
                # print(f"Label: {child.get_label()}")
                s[child.get_label()] = {}
                for sub_child in child.get_children():
                    if(name):
                        pass
                        # print(f">{sub_child.get_name()}: {sub_child.get_value()}")
                    else:
                        s[child.get_label()][sub_child.get_label()] = sub_child.get_value()
                        # print(f">{sub_child.get_label()}: {sub_child.get_value()}")

            # status=config.get_child_by_name('status')
            # widget=status.get_child_by_name('batterylevel')
            # s['Other PTP Device Properties']['battery_level'] = widget.get_value()
            # s['Other PTP Device Properties']['Summary'] = self.camera.get_summary()

            # Set particular values...
            s['iso'] = s['Image Settings']['ISO Speed'] if 'ISO Speed' in s['Image Settings'] else None
            s['aperture'] = s['Capture Settings']['F-Number']
            s['shutter_speed'] = s['Capture Settings']['Shutter Speed']
            si=self.camera.get_storageinfo()
            # print(f"Storage info: {si[0]}, {si[0].fields} {si[0].freekbytes}, {si[0].capacitykbytes}")
            s['card_usage_1'] = 0
            s['card_usage_2'] = 0
            s['card_capacity_1'] = 0
            s['card_capacity_2'] = 0
            if(len(si)==0):
                s['card_present_1'] = False
                s['card_present_2'] = False
            elif(len(si)==1):
                s['card_present_1'] = True
                s['card_present_2'] = False
            else:
                s['card_present_1'] = True
                s['card_present_2'] = True
            if(len(si)>0):    
                s['card_usage_1'] = round(100*(1-float(si[0].freekbytes)/float(si[0].capacitykbytes)))
                s['card_capacity_1'] = round(float(si[0].capacitykbytes)/1048576)
            if(len(si)>1):
                s['card_usage_2'] = round(100*(1-float(si[1].freekbytes)/float(si[1].capacitykbytes)))
                s['card_capacity_2'] = round(float(si[1].capacitykbytes)/1048576)
        
            return s
                    
        except gp.GPhoto2Error as e:
            print(f"Error getting main configuration settings: {e}")
    
    # Debugging info to sniff the parameters
    def get_settings_print(self, name=False):
        try:
            # Get the main configuration
            #only needs to be called once and it updates a internal widget list
            config = self.camera.get_config()
            

            # Print the main configuration settings
            for child in config.get_children():
                print(f"Label: {child.get_label()}")
                for sub_child in child.get_children():
                    if(name):
                        print(f">{sub_child.get_name()}: {sub_child.get_value()}")
                    else:
                        print(f">{sub_child.get_label()}: {sub_child.get_value()}")
                    
        except gp.GPhoto2Error as e:
            print(f"Error getting main configuration settings: {e}")

    def set_Camera_Setting(self, setting, value):
        try:
            config = self.camera.get_config()  
            self.set_parameter_by_label(setting, value, config)
        except gp.GPhoto2Error as e:
            print(f"Error setting camera setting: {e}")

    def move_focus(self, distance):
        try:
            config = self.camera.get_config()
            focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
            value = int(focus_step_widget.get_value())
            value += int(distance)
            focus_step_widget.set_value(value)
            self.camera.set_config(config)
        except gp.GPhoto2Error as e:
            print(f"Error moving focus: {e}")

    def set_focus(self, distance):
        try:
            config = self.camera.get_config()
            focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
            focus_step_widget.set_value(distance)
            self.camera.set_config(config)
        except gp.GPhoto2Error as e:
            print(f"Error moving focus: {e}")

    def perform_focus_limit_test(self, doublecheck = False):
        print("starting focus limit test")
        final_focus_z = 0
        double_check_z = 0

        def move_focus_step(step):
            config = self.camera.get_config()
            focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
            value = int(focus_step_widget.get_value())
            value += step
            focus_step_widget.set_value(value)
            self.camera.set_config(config)

        def back_up_to_start():
            for step in range(10, 0, -1):
                while True:
                    try:
                        move_focus_step(-step)
                    except gp.GPhoto2Error:
                        if step == 1:
                            print("backed up to start")
                        else:
                            print(f"finishing {step}")
                        break

        def find_focus_limit():
            nonlocal final_focus_z
            for step in range(10, 0, -1):
                while True:
                    try:
                        move_focus_step(step)
                        final_focus_z += step
                    except gp.GPhoto2Error:
                        if step == 1:
                            print(f"Focus limit reached at {final_focus_z}")
                        else:
                            print(f"Error moving focus with step = {step}, trying {step - 1}")
                        break

        def double_check_focus_limit():
            nonlocal double_check_z
            for step in range(10, 0, -1):
                while True:
                    try:
                        move_focus_step(-step)
                        double_check_z += step
                    except gp.GPhoto2Error:
                        if step == 1:
                            print(f"Focus limit reached at {double_check_z}")
                        else:
                            print(f"Error moving focus with step = {step}, trying {step - 1}")
                        break

        try:
            back_up_to_start()
            find_focus_limit()
            if doublecheck:
                double_check_focus_limit()#also gets focus back to start at 0 for PAState
        except gp.GPhoto2Error as e:
            print(f"Error moving focus: {e}")

        if final_focus_z == double_check_z or doublecheck == False:    
            self.max_z_focus = final_focus_z
            return final_focus_z
        else:
            print("error calibrating lens, forward and back didnt match")
            return False
    
    def get_max_focus_z(self):
        return self.max_z_focus
    
    def get_current_focus(self):
        config = self.camera.get_config()
        focus_step_widget = config.get_child_by_label("Drive Nikon DSLR Manual focus")
        return int(focus_step_widget.get_value())