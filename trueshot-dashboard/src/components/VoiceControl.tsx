import { useEffect } from 'react';
import { toast } from 'react-hot-toast';
import SpeechRecognition, { useSpeechRecognition } from 'react-speech-recognition';
import { Mic, MicOff } from 'lucide-react';

export const VoiceControl = ({ onStartScan, onStopScan }: { onStartScan: () => void, onStopScan: () => void }) => {
    const { transcript, listening, resetTranscript, browserSupportsSpeechRecognition } = useSpeechRecognition();

    useEffect(() => {
        if (!browserSupportsSpeechRecognition) return;

        const cmd = transcript.toLowerCase();
        if (cmd.includes('start scan') || cmd.includes('begin sequence')) {
            toast.success("Voice Command Recieved: START");
            onStartScan();
            resetTranscript();
        } else if (cmd.includes('stop') || cmd.includes('abort') || cmd.includes('halt')) {
            toast.error("Voice Command Recieved: ABORT");
            onStopScan();
            resetTranscript();
        }
    }, [transcript, onStartScan, onStopScan, resetTranscript, browserSupportsSpeechRecognition]);

    if (!browserSupportsSpeechRecognition) return null;

    return (
        <button
            onClick={() => listening ? SpeechRecognition.stopListening() : SpeechRecognition.startListening({ continuous: true })}
            className={`fixed bottom-6 right-20 z-50 p-4 rounded-full transition-all shadow-xl border border-white/10 ${listening ? 'bg-red-500/20 text-red-500 animate-pulse' : 'bg-black/50 text-white/50 hover:bg-black/80 hover:text-white'}`}
            title="Voice Control"
        >
            {listening ? <Mic className="w-5 h-5" /> : <MicOff className="w-5 h-5" />}
        </button>
    );
};
