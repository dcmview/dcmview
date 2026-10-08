# The "remote server" for the remote-ssh check profile: an SSH host with
# Python and a normal user, and no dcmview. The suite copies in the wheel
# under test and the user's key at run time.
ARG BASE_IMAGE=ubuntu:22.04
FROM ${BASE_IMAGE}

RUN apt-get update \
	&& DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
		openssh-server python3 python3-venv procps \
	&& rm -rf /var/lib/apt/lists/* \
	&& mkdir -p /run/sshd \
	&& useradd --create-home --shell /bin/bash annot \
	&& install -d -m 700 -o annot -g annot /home/annot/.ssh \
	&& printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nAllowTcpForwarding yes\nAllowStreamLocalForwarding yes\n' \
		> /etc/ssh/sshd_config.d/remote-ssh-check.conf

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
