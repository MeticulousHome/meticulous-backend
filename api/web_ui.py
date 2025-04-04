import tornado.ioloop
import tornado.web
import tornado.escape
import os

WEB_UI_HANDLER = [
    (r"", tornado.web.RedirectHandler, {"url": "/"}),
    (r"/graph", tornado.web.RedirectHandler, {"url": "/debug/graph.html"}),
    (r"/debug", tornado.web.RedirectHandler, {"url": "/debug/graph.html"}),
    (
        r"/debug/(.*)",
        tornado.web.StaticFileHandler,
        {
            "default_filename": "graph.html",
            "path": os.path.dirname(__file__) + "/web_ui",
        },
    ),
]
