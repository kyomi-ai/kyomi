#!/usr/bin/env python3
"""
Integration test for Python → Node chart renderer

Simulates the real Slack alert flow:
1. Python queries database (mocked with sample data)
2. Python builds ChartML spec with inline data
3. Python POSTs to Node renderer service
4. Node renders to PNG
5. Python saves PNG
"""

import httpx
import asyncio
import json
import base64
from pathlib import Path

CHART_RENDERER_URL = "http://localhost:3030"

async def test_chart_rendering_integration():
    """Simulate the real Slack alert flow"""

    print("🧪 Testing Full Integration: Python → Node Chart Renderer\n")

    # Step 1: Simulate database query result
    # In real code, this would come from BigQuery/Postgres/etc
    print("Step 1: Query database (simulated)")
    query_result = [
        {"country_code": "US", "sessions": 1247},
        {"country_code": "GB", "sessions": 892},
        {"country_code": "CA", "sessions": 654},
        {"country_code": "DE", "sessions": 543},
        {"country_code": "FR", "sessions": 421},
        {"country_code": "AU", "sessions": 312},
        {"country_code": "JP", "sessions": 287},
    ]
    print(f"✅ Got {len(query_result)} rows from database\n")

    # Step 2: Build ChartML spec with inline data
    # This is what the Python backend would do
    print("Step 2: Build ChartML spec with inline data")
    chartml_spec = {
        "type": "chart",
        "version": 1,
        "title": "Sessions by Country",
        "data": {
            "provider": "inline",
            "rows": query_result  # Database results go here
        },
        "visualize": {
            "type": "bar",
            "columns": "country_code",
            "rows": "sessions",
            "axes": {
                "left": {
                    "label": "Sessions",
                    "format": ",.0f"
                }
            },
            "style": {
                "height": 400
            }
        }
    }
    print(f"✅ Built ChartML spec with {len(query_result)} data rows\n")

    # Step 3: Call Node chart renderer service
    print("Step 3: Call chart renderer service")
    async with httpx.AsyncClient() as client:
        try:
            response = await client.post(
                f"{CHART_RENDERER_URL}/render",
                json={
                    "chartMLSpec": chartml_spec,  # Can pass object or YAML string
                    "width": 800,
                    "height": 400
                },
                timeout=30.0
            )
            response.raise_for_status()
            result = response.json()

            if "error" in result:
                print(f"❌ Renderer error: {result['error']}")
                return False

            print(f"✅ Got PNG image ({len(result['image'])} chars base64)\n")

            # Step 4: Save PNG (in real code, would upload to Slack)
            print("Step 4: Save PNG image")
            output_dir = Path(__file__).parent / "test-charts"
            output_dir.mkdir(exist_ok=True)
            output_file = output_dir / "integration-test.png"

            png_data = base64.b64decode(result["image"])
            output_file.write_bytes(png_data)
            print(f"✅ Saved to: {output_file}\n")

            # Step 5: Verify
            print("Step 5: Verify PNG file")
            file_size = len(png_data)
            print(f"✅ PNG size: {file_size:,} bytes")

            if file_size < 1000:
                print("⚠️  Warning: PNG is very small, might be empty")
                return False

            print(f"\n✅ INTEGRATION TEST PASSED")
            print(f"\nGenerated chart from database query:")
            print(f"  - Database rows: {len(query_result)}")
            print(f"  - ChartML spec: {len(json.dumps(chartml_spec))} bytes")
            print(f"  - PNG output: {file_size:,} bytes")
            print(f"  - File: {output_file}")
            print(f"\nOpen the file to verify the chart rendered correctly!")

            return True

        except httpx.ConnectError:
            print(f"❌ Could not connect to chart renderer at {CHART_RENDERER_URL}")
            print(f"💡 Start the service: node src/server.js")
            return False
        except Exception as e:
            print(f"❌ Error: {e}")
            return False

if __name__ == "__main__":
    success = asyncio.run(test_chart_rendering_integration())
    exit(0 if success else 1)
