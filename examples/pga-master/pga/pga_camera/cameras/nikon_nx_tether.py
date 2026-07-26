# Indirect control through Nikon's NXTether software with USB connection, requires AppleScript to control the software GUI



from ..util.apple_script import AppleScript  

class NikonNXTether:
    def __init__(self, aperture=None, shutter_speed=None, iso=None, shots=None, manual_refocus=False, ascript=None):
        self.aperture = aperture
        self.shutter_speed = shutter_speed
        self.iso = iso
        self.shots = shots
        self.manual_refocus = manual_refocus
        self.initialize_camera(ascript)

    def _exec_applescript(self, scpt, ascript=None):
        if(ascript):
            ascript.chain(scpt)
        else:
            AppleScript.run(scpt)

    # This script is specific to Z9 camera as it is clicking on a window with that nane
    def initialize_camera(self, ascript=None):
        pass
        # scpt = """
        #     -- Click “NX Tether” in the Dock.
        #     tell application "System Events"
        #         click UI element "NX Tether" of list 1 of application process "Dock"
        #         -- Bring the window “Z 9” to the front.
        #         click window "Z 9" of application process "NX Tether"
        #         --Bring the window “Z 9” to the front.
        #         click window "Z 9" of application process "NX Tether"
        #         set dialogReply to display dialog "Press OK when camera has been set up" buttons {"OK"} default button "OK" giving up after 60
        #         --return dialogReply
        #     end tell
        # """
        # self._exec_applescript(scpt, ascript)
    
    def set_aperture(self, aperture):
        self.aperture = aperture

    def set_shutter_speed(self, shutter_speed):
        self.shutter_speed = shutter_speed

    def set_iso(self, iso):
        self.iso = iso

    def set_shots(self, shots):
        self.shots = shots

    def perform_focus_shifted_shot_sequence(self, ascript=None):
        if(config.dev_mode):
            for _ in range(self.shots):
                time.sleep(1)
                print(".",end='')
        else:
            scpt = '''
                tell application "System Events"
                    click UI element "NX Tether" of list 1 of application process "Dock"
                    -- Bring the window “Z 9” to the front.
                    click window "Z 9" of application process "NX Tether"
                    --Bring the window “Z 9” to the front.
                    click window "Z 9" of application process "NX Tether"
                    set dialogReply to display dialog "Press 'OK' when camera has been set up" buttons {"OK"} default button "OK" giving up after 60
                    --return dialogReply
                --end tell

                --tell application "System Events"
                    tell application process "NX Tether"
                        --click UI element 36 of window "Z 9" of application process "NX Tether"
                        delay 0.5
                        click UI element "Start" of window "Detached Window"
                        repeat until (exists (button "Start" of window "Detached Window"))
                            delay 0.1
                        end repeat
                    end tell
                end tell
            '''
           
            # Chain the manual reset_focus if needed
            if(self.manual_refocus):
                ascript=ascript if ascript else AppleScript.batch(scpt)
                self.reset_focus(ascript=ascript)
            else:
                self._exec_applescript(scpt, ascript)


    # Currently not working
    def reset_focus(self, steps=None, ascript=None):
        steps = steps if steps else self.shots
        scpt = f'''
                tell application "System Events"
                    tell application process "NX Tether"
                        --back up the focus
                        set clicks to {steps}
                        repeat with i from 1 to clicks
                            delay 0.1
                            click UI element 36 of window "Z 9" of application process "NX Tether"
                        end repeat
                    end tell
                end tell
            '''
        self._exec_applescript(scpt, ascript)

    def open_exposure_wndow(self, ascript=None):
        scpt = """
                    -- open exposure window
                    tell application "System Events" to click checkbox "0.0" of group 5 of list 1 of list 1 of scroll area 1 of window "Z 9" of application process "NX Tether"
            """

        self._exec_applescript(scpt, ascript)


    def open_focus_shift_window(self, ascript=None):
        scpt = """
                    -- open focus shift window
                    tell application "System Events" to click checkbox "Focus shift" of group 2 of list 3 of list 1 of scroll area 1 of window "Z 9" of application process "NX Tether"
            """
        self._exec_applescript(scpt, ascript)  


    def subtract_point_3_exposure(self, ascript=None):
        scpt = """
                    -- exposure -.33
                    tell application "System Events" to click checkbox 2 of window "Detached Window" of application process "NX Tether"
            """
        self._exec_applescript(scpt, ascript)

    def subtract_1_exposure(self, ascript=None):
        scpt = """
                    -- exposure -1
                    tell application "System Events" to click checkbox 1 of window "Detached Window" of application process "NX Tether"
            """
        self._exec_applescript(scpt, ascript)

    def add_point_3_exposure(self, ascript=None):
        scpt = """
                    -- exposure +.33
                    tell application "System Events" to click checkbox 3 of window "Detached Window" of application process "NX Tether"
            """
        
        self._exec_applescript(scpt, ascript)

    def add_1_exposure(self, ascript=None):
        scpt = """
                    -- exposure +1
                    tell application "System Events" to click checkbox 4 of window "Detached Window" of application process "NX Tether"
            """
        self._exec_applescript(scpt, ascript)

    def calibrate_lens_focus():
        # calibrate the lens focus, may require threading...
        print("Calibrating lens focus")
        pass


