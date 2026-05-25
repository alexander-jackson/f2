from flask import Flask

app = Flask(__name__)


@app.route("/<content>")
def echo(content):
    return f"Echo echo {content}", 200


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=8080)
