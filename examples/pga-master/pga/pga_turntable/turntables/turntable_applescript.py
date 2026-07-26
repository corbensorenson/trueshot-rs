from ...util.apple_script import AppleScript


class TurntableApplescript:
    def __init__(self, rotation_step, ascript=None):
        scpt = f"""
                tell application "System Events"
                    tell application "foldio360 Control" to activate
                    --click UI element "foldio360 Control" of list 1 of application process "Dock"
                    delay 0.5
                    tell application process "foldio360 Control"
                        set theField to text field 1 of window 1
                        set focused of theField to true
                        click theField
                        delay 0.3
                        click theField
                        key code 124 -- right arrow
                        key code 124
                        key code 124
                        delay 0.5
                        -- command delete -> hilights the text to the left of the cursor
                        key code 51 using {{command down}}
                        delay 0.5
                        keystroke "{rotation_step}"
                    end tell
                end tell
        """
       
        self._exec_applescript(scpt, ascript)


    def rotate(self, degrees, wait=True, ascript=None):
        # Implement the rotate method for the Folio implementation - this will ignore the degrees argument
        # scp=None
        scpt="""
                        set input to 1
                        -- push right button
                        tell application "System Events" 
                            click UI element "btn-rotate-delta-right" of window "foldio360 Control" of application process "foldio360 Control"                           
                            delay 0.5
                            tell application process "foldio360 Control"
                                repeat
                                    delay 0.1
                                    if not (exists (button "STOP" of window "foldio360 Control")) then exit repeat
                                end repeat
                            end tell
                        end tell
                        return input
            """ if (wait) else """
                                -- push right button
                                tell application "System Events" to click UI element "btn-rotate-delta-right" of window "foldio360 Control" of application process "foldio360 Control"
                    """
        self._exec_applescript(scpt, ascript)

    def _exec_applescript(self, scpt, ascript=None):
        if(ascript):
            ascript.chain(scpt)
        else:
            AppleScript.run(scpt)