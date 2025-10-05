#!/bin/bash

# Load API key from .env
source .env

echo "Testing LunaRoute proxy with API key..."
echo

# Test the proxy
curl -v -X POST http://127.0.0.1:8081/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "Say hello in exactly 3 words"}],
    "max_tokens": 20
  }'

echo
echo "Done!"
