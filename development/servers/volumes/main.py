from flask import Flask

app = Flask(__name__)


@app.route("/")
def root():
    # Read the content of the file at `/data/configuration.json`
    with open("/data/configuration.json", "r") as f:
        content = f.read()

    # Return it with the content type set to `application/json`
    return content, 200, {"Content-Type": "application/json"}


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=8080)
