import subprocess
import sys
from subprocess import Popen, PIPE

class AppleScript:
    def __init__(self, script_text):
        self.script_text = script_text
        
    @classmethod
    def run(cls, script_text=""):
        applescript = cls(script_text)
        applescript.execute()
        return applescript
    
    @classmethod
    def batch(cls, script_text=""):
        applescript = cls(script_text)
        return applescript

    def chain(self, script_text):
        self.script_text+="\n"+script_text

    def execute(self):
        s="on run\n"+self.script_text+"\nend run"
        # print(s)
        try:
            subprocess.run(['osascript', '-e', s], check=True, stderr=subprocess.PIPE)
        except subprocess.CalledProcessError as e:
            print(e.stderr.decode(), file=sys.stderr)

    # def run_apple_script(self):
    #     scpt=self.script_text
    #     args=self.args

    #     print(scpt)
    #     if isinstance(scpt, str):
    #         scpt = scpt.encode('utf-8')
    #     # print(f"Type of scpt after encoding: {type(scpt)}")
    #     # print(f"Script: {scpt}")
    #     # print(f"Args: {args}")

    #     try:
    #         command = ['osascript', '-'] + args if args else ['osascript', '-']
    #         p = Popen(command, stdin=PIPE, stdout=PIPE, stderr=PIPE)
    #         stdout, stderr = p.communicate(input=scpt)
            
    #         print("Subprocess completed")
        
    #         print(f"Return code: {p.returncode}")
    #         print(f"stdout: {stdout.decode('utf-8')}")
    #         print(f"stderr: {stderr.decode('utf-8')}")
    #     except OSError as e:
    #         print("Execution failed:", e, file=sys.stderr)