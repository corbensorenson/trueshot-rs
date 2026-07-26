import React from 'react'
import ReactDOM from 'react-dom/client'
import { BrowserRouter, Routes, Route } from 'react-router-dom'
import App from './App'
import { GuestPortalWrapper } from './components/GuestPortalWrapper'
import SlavePhone from './components/SlavePhone'
import EventDashboard from './components/EventDashboard'
import PhotoEditor from './components/PhotoEditor'
import CameraControlPro from './components/CameraControlPro'
import './index.css'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter>
      <Routes>
        {/* Main Application */}
        <Route path="/" element={<App />} />

        {/* Photo Editor Mode */}
        <Route path="/editor" element={<PhotoEditor />} />

        {/* Advanced Camera Control */}
        <Route path="/camera" element={<CameraControlPro />} />

        {/* Guest Portal - for event attendees */}
        <Route path="/guest/:eventId" element={<GuestPortalWrapper />} />

        {/* Slave Phone - server-controlled camera */}
        <Route path="/slave" element={<SlavePhone />} />
        <Route path="/slave/:sessionName" element={<SlavePhone />} />

        {/* Event Dashboard - for organizers */}
        <Route path="/events" element={<EventDashboard />} />
      </Routes>
    </BrowserRouter>
  </React.StrictMode>,
)
