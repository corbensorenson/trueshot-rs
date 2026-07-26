#!/bin/bash
set -euo pipefail

# Configuration
# BACKEND_PORT=8000
# FRONTEND_PORT=5173
PROJECT_ROOT=$(pwd)
PID_FILE="$PROJECT_ROOT/.trueshot_pids"

# Function to cleanup background processes on exit
cleanup() {
    echo "Stopping TrueShot..."
    if [ -f "$PID_FILE" ]; then
        while IFS= read -r line; do
            if [[ "$line" =~ ^(backend|frontend):([0-9]+)$ ]]; then
                pid="${BASH_REMATCH[2]}"
                if kill -0 "$pid" 2>/dev/null; then
                    kill "$pid" 2>/dev/null || true
                fi
            fi
        done < "$PID_FILE"
        rm -f "$PID_FILE"
    fi
    kill $(jobs -p) 2>/dev/null || true
    exit
}

trap cleanup SIGINT SIGTERM EXIT

echo "Starting TrueShot Launcher..."

# 1. Stop previous launcher instances (safe)
if [ -f "$PID_FILE" ]; then
    echo "Stopping previous TrueShot processes..."
    while IFS= read -r line; do
        if [[ "$line" =~ ^(backend|frontend):([0-9]+)$ ]]; then
            pid="${BASH_REMATCH[2]}"
            if kill -0 "$pid" 2>/dev/null; then
                kill "$pid" 2>/dev/null || true
                sleep 0.2
            fi
        fi
    done < "$PID_FILE"
    rm -f "$PID_FILE"
fi

if lsof -ti:3000 >/dev/null 2>&1; then
  echo "Port 3000 is already in use. Stop the other service before launching."
  exit 1
fi
if lsof -ti:5173 >/dev/null 2>&1; then
   echo "Port 5173 is already in use. Stop the other service before launching."
   exit 1
fi

# 2. Build Backend (with error checking)
echo "Building Backend..."
cd "$PROJECT_ROOT"
if ! cargo build -p trueshot-server 2>&1 | tee build.log; then
    echo "ERROR: Backend build failed! Check build.log for details."
    exit 1
fi
echo "Backend build successful."

# 3. Start Backend
echo "Starting Backend Server..."
RUST_LOG=info cargo run -p trueshot-server > backend.log 2>&1 &
BACKEND_PID=$!
echo "backend:${BACKEND_PID}" > "$PID_FILE"

# 3. Start Frontend
echo "Starting Frontend Dashboard..."
cd "$PROJECT_ROOT/trueshot-dashboard"
npm run dev > ../frontend.log 2>&1 &
FRONTEND_PID=$!
echo "frontend:${FRONTEND_PID}" >> "$PID_FILE"

# 4. Wait for Services
echo "Waiting for services to initialize..."

wait_for_service() {
    local url=$1
    local name=$2
    local count=0
    while ! curl -s $url > /dev/null; do
        sleep 1
        count=$((count+1))
        echo "Waiting for $name... ($count s)"
        if [ $count -gt 120 ]; then
            echo "$name failed to start. Check backend.log for details."
            exit 1
        fi
    done
    echo "$name is Ready!"
}

# Wait for Backend (API Health) - Port 3000
wait_for_service "http://localhost:3000/api/health" "Backend API"

# Wait for Frontend
wait_for_service "http://localhost:5173" "Frontend Dashboard"

# 5. Launch Browser
echo "Launching Browser..."
if command -v open >/dev/null 2>&1; then
  open "http://localhost:5173"
fi

echo "TrueShot is Running."
echo "Press Ctrl+C to stop."

# Wait indefinitely
wait $BACKEND_PID $FRONTEND_PID
