#!/bin/bash
# TrueShot Development Startup Script
# Runs both backend and frontend in development mode

set -e

# Kill any existing processes on our ports
echo "🔄 Cleaning up existing processes..."
lsof -ti:3000 | xargs kill -9 2>/dev/null || true
lsof -ti:5173 | xargs kill -9 2>/dev/null || true

echo "🚀 Starting TrueShot Development Environment"
echo "============================================"

# Start backend
echo "📡 Starting backend server..."
cargo run -p trueshot-server &
BACKEND_PID=$!

# Wait for backend to start
sleep 3

# Start frontend
echo "🎨 Starting frontend dev server..."
cd trueshot-dashboard
npm run dev &
FRONTEND_PID=$!
cd ..

echo ""
echo "✅ Development servers running:"
echo "   Backend:  http://localhost:3000"
echo "   Frontend: http://localhost:5173"
echo "   API Docs: http://localhost:3000/api/docs"
echo ""
echo "Press Ctrl+C to stop all servers"

# Wait for interrupt
trap "kill $BACKEND_PID $FRONTEND_PID 2>/dev/null; exit 0" INT
wait
