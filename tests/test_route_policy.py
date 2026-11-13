import importlib.util
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "tools/route-policy.py"
spec = importlib.util.spec_from_file_location("route_policy", SCRIPT)
route_policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(route_policy)


class RoutePolicyTableTests(unittest.TestCase):
    def test_the_table_lists_exactly_the_registered_routes(self):
        self.assertEqual(route_policy.main([]), 0)
        rows = route_policy.read_table()
        self.assertGreater(len(rows), 400)
        self.assertNotIn("?", rows.values(), "every route has a reviewed verdict")
        self.assertFalse([route for _, route in rows if route.startswith("/api/plugins/")])

    def test_routes_are_read_from_production_code_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates/demo/src"
            (source / "fixtures").mkdir(parents=True)
            (source / "lib.rs").write_text(
                'mod api;\n#[cfg(test)]\nmod fixtures;\n'
                'fn routes() { Router::new().route("/api/demo/{id}", get(read).put(save))\n'
                '    .route("/healthz", get(health))\n'
                '    .route(\n        "/api/demo/{id}/run",\n        post(run).layer(limit(8)),\n    ); }\n'
                '#[cfg(test)]\nmod tests { fn mock() { Router::new().route("/api/mock", get(x)); } }\n'
            )
            (source / "api.rs").write_text(
                'use axum::routing::{get as route_get, post};\n'
                'fn more() { Router::new().route("/api/demo", post(create))\n'
                '    .route("/api/demo/{id}/items", route_get(get).delete(remove)); }\n'
            )
            (source / "fixtures/mod.rs").write_text('fn server() { Router::new().route("/api/fake", get(x)); }\n')
            (source / "api_tests.rs").write_text('fn t() { Router::new().route("/api/tested", get(x)); }\n')
            self.assertEqual(
                route_policy.registered(root),
                {
                    ("GET", "/api/demo/{id}"),
                    ("PUT", "/api/demo/{id}"),
                    ("POST", "/api/demo/{id}/run"),
                    ("POST", "/api/demo"),
                    ("GET", "/api/demo/{id}/items"),
                    ("DELETE", "/api/demo/{id}/items"),
                },
            )

    def test_writing_keeps_reviewed_verdicts_and_marks_new_routes(self):
        with tempfile.TemporaryDirectory() as directory:
            table = Path(directory) / "table.tsv"
            route_policy.write_table({("GET", "/api/b"): "kept", ("POST", "/api/a"): "?"}, table)
            self.assertEqual(
                table.read_text().splitlines()[1:],
                ["POST\t/api/a\t?", "GET\t/api/b\tkept"],
            )
            self.assertEqual(route_policy.read_table(table), {("GET", "/api/b"): "kept", ("POST", "/api/a"): "?"})


if __name__ == "__main__":
    unittest.main()
