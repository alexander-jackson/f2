# `f2`

`f2` is a basic container orchestration system and load balancer, aimed at
making simple continuous deployments easier.

### What does it do?

The program handles 2 main things for you:
* Discovering and rolling new Docker images onto a host
* Load balancing requests across the available containers

This means you only need to build a new Docker image for your downstream
client. `f2` will handle pulling that onto the server, spinning up the new
version and ensuring it is healthy and then redirecting traffic to the new
container before terminating the old one. This allows for zero downtime rolling
upgrades of the server.

`f2` is targeted at developers who want the benefits of continous deployments
for smaller scale applications. If you are running an HTTP server on an EC2
instance for example, `f2` will allow you to Dockerise that application and
benefit from automated deployments whenever you build a new image, without
causing requests to fail for users.

### What does it not do?

`f2` is (by design) much simpler than something like Kubernetes. It does not
provide the same guarantees in terms of fault tolerance and health checking.

It doesn't provide components like ingress management, certificate handling or
horizontal pod autoscaling. If you need these, `f2` is not appropriate (at the
moment) for your use case.
