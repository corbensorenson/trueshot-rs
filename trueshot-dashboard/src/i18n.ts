import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

// Minimal resources
const resources = {
    en: {
        translation: {
            "Start Scan": "Start Scan",
            "Stop": "Stop",
            "Processing": "Processing...",
            "CalibrationWizard": "Calibration Wizard"
        }
    },
    es: {
        translation: {
            "Start Scan": "Iniciar Escaneo",
            "Stop": "Parar",
            "Processing": "Procesando...",
            "CalibrationWizard": "Asistente de Calibración"
        }
    }
};

i18n
    .use(initReactI18next)
    .init({
        resources,
        lng: "en",
        interpolation: {
            escapeValue: false
        }
    });

export default i18n;
